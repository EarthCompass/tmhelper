use futures::sink::SinkExt;
use futures::stream::SplitStream;
use futures::{stream::SplitSink, StreamExt};
use futures::{select, FutureExt};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio::time;
use tokio_tungstenite::{
    connect_async, tungstenite::protocol::Message, MaybeTlsStream, WebSocketStream,
};

const CONNECTION: &'static str = "wss://www.teachermate.com.cn/faye";


#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct SignEvent {
    pub course_id: u64,
    pub sign_id: u64,
	// #[serde(rename = "isGPS")]
    // is_gps: u8,
	#[serde(rename = "isQR")]
    is_qr: u8,
    // name: String,
    // code: String,
    // start_year: u16,
    // term: String,
    // cover: String,
}
#[derive(Deserialize)]
struct ErrMsg {
    message: String,
}
struct QrSignState {
    seqid: u64,
    clientid: String,
    courseid: u64,
    signid: u64,
}
struct QrSign {
    state: Arc<Mutex<QrSignState>>,

    read: SplitStream<WebSocketStream<MaybeTlsStream<TcpStream>>>,
    write: SplitSink<WebSocketStream<MaybeTlsStream<TcpStream>>, Message>,
}

impl QrSignState {
    fn get_seqid(&mut self) -> u64 {
        self.seqid += 1;
        self.seqid
    }
}
impl<'a> QrSign {
    pub async fn from_sign_event(sign_event: SignEvent) -> QrSign {
        let (ws_stream, _) = connect_async(CONNECTION).await.expect("Failed to connect");
        let (write, read) = ws_stream.split();
        let arc = Arc::new(Mutex::new(QrSignState {
            seqid: 0,
            clientid: "".to_string(),
            courseid: sign_event.course_id,
            signid: sign_event.sign_id,
        }));
        QrSign {
            state: arc,
            read,
            write,
        }
    }

    pub async fn handshake(&mut self) {
        let handshake_msg = json!({
            "channel": "/meta/handshake",
            "version": "1.0",
            "supportedConnectionTypes": [
                "websocket",
                "eventsource",
                "long-polling",
                "cross-origin-long-polling",
                "callback-polling",
              ],
            "id":self.state.lock().await.get_seqid()
        });
        self.write
            .send(Message::Text(handshake_msg.to_string()))
            .await
            .unwrap();
    }

    pub async fn handle_msg(mut self,urlmap:Arc<Mutex<HashMap<String, String>>>) {
        let state = self.state.lock().await;
        let mut interval = time::interval(Duration::from_millis(7500));
        let mut cnt = 0;
        let qr_channel = format!("{}/{}", state.courseid, state.signid);
        drop(state);
        'lp: loop {
            select! {
                message = self.read.next().fuse() => {
                    let msg = message.unwrap().unwrap();
					if msg.is_close() {
						break 'lp;
					}

                    let data = msg.into_text().unwrap();
                    let vs: Value = serde_json::from_str(&data).unwrap();
                    if !vs.is_array() || vs.as_array().unwrap().len() == 0 {
                        continue 'lp;
                    }
                    let v = &vs[0];
                    match v["channel"].as_str() {
                        Some("/meta/handshake") => {
                            log::info!("clientid: {}", v["clientId"].as_str().unwrap().to_string());
                            self.state.lock().await.clientid = v["clientId"].as_str().unwrap().to_string();
                            self.connect().await;
                        },
                        Some("/meta/connect") => {
                            log::debug!("connect");
                            self.subscribe().await;
                        },
                        Some("/meta/subscribe") => {
                            log::debug!("subscribe");
                        },
                        _ => {
                            cnt = 0;
                            let qr_url = v["data"]["qrUrl"].as_str().unwrap().to_string();
							urlmap.lock().await.insert(qr_channel.clone(), qr_url.clone());
                            log::info!("qrUrl: {}->{}", qr_channel,qr_url);
                        }
                    }
                },
                _ = interval.tick().fuse() => {
                    if cnt >5{
                        log::info!("{} timeout",qr_channel);
                        break 'lp;
                    }
                    cnt += 1;
                    self.heartbeat().await;
                }
            }
        }
		urlmap.lock().await.remove(&qr_channel);
		log::info!("{} removed",qr_channel);
    }
    pub async fn connect(&mut self) {
        let msg = json!({
            "channel": "/meta/connect",
            "clientId": self.state.lock().await.clientid,
            "connectionType": "websocket",
            "id": self.state.lock().await.get_seqid()
        });
        self.write
            .send(Message::Text(msg.to_string()))
            .await
            .unwrap();
    }
    pub async fn subscribe(&mut self) {
        let mut state = self.state.lock().await;
        let msg = json!({
            "channel": "/meta/subscribe",
            "clientId": state.clientid,
            "subscription": format!("/attendance/{}/{}/qr",state.courseid,state.signid),
            "id": state.get_seqid()
        });
        self.write
            .send(Message::Text(msg.to_string()))
            .await
            .unwrap();
    }
    pub async fn heartbeat(&mut self) {
        self.write
            .send(Message::Text("[]".to_string()))
            .await
            .unwrap();
        self.connect().await;
    }
}

pub async fn get_sign(openid: String) -> Result<SignEvent, Box<dyn std::error::Error>> {
    let client = reqwest::Client::new();
    let res = client
        .get("https://v18.teachermate.cn/wechat-api/v1/class-attendance/student/active_signs")
        .query(&[("openid", openid)])
        .send()
        .await?;
    if res.status() != 200 {
        let errmsg = res.json::<ErrMsg>().await?;
        return Err(errmsg.message.into());
    }
    let signs = res.json::<Vec<SignEvent>>().await?;
    if signs.len() == 0 {
        return Err("no sign".into());
    }
	for sign in signs.iter(){
		if sign.is_qr == 1{
			return Ok(sign.clone());
		}
	}
	Err("no QR sign".into())
}
pub async fn qrsign(sign: SignEvent,urlmap:Arc<Mutex<HashMap<String, String>>>) {
    let mut qr_sign = QrSign::from_sign_event(sign).await;
    qr_sign.handshake().await;
    qr_sign.handle_msg(urlmap).await;
}
