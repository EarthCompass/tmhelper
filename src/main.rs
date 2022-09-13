use std::{collections::HashMap, sync::Arc};

use actix_web::{get, web, App, HttpResponse, HttpServer, Responder};
use log::info;
use tokio::{sync::Mutex, task};
mod tmsign;

struct UrlMapState {
    url_map: Arc<Mutex<HashMap<String, String>>>,
}

#[get("/openid/{openid}")]
async fn hello(openid: web::Path<String>,data: web::Data<UrlMapState>) -> impl Responder {
    // println!("{:?}",tmsign::get_sign().await.unwrap());
    log::info!("incoming openid: {}", openid);
    let sign = tmsign::get_sign(openid.to_string()).await;
    if sign.is_ok() {
        let sign_event = sign.unwrap();
        task::spawn(
            // time::sleep(time::Duration::from_secs(5)).await;
            // println!("Hello from blocking task");
            tmsign::qrsign(sign_event.clone(),data.url_map.clone()),
        );
        return HttpResponse::Ok().body(format!("{}/{}", sign_event.course_id, sign_event.sign_id));
        // return HttpResponse::Ok().body(format!("{:?}!", sign_event));
    } else {
        return HttpResponse::BadRequest().body(format!("{}!", sign.unwrap_err()));
    }
}
#[get("/redirect/{courseid}/{signid}")]
async fn redirect(info: web::Path<(String,String)>,data: web::Data<UrlMapState>) -> impl Responder {
    let qr_channel = format!("{}/{}", info.0, info.1);
    let map = data.url_map.lock().await;
    let url = map.get(&qr_channel);
    // println!("{}",qr_channel);
    if url.is_some() {
        log::info!("redirect: {}->{}", qr_channel,url.unwrap());
        return HttpResponse::Found().append_header(("Location", url.unwrap().as_str())).finish();
    } else {
        log::info!("redirect not found: {}", qr_channel);
        return HttpResponse::NotFound().body(format!("{}!", "not found"));
    }
}
#[actix_web::main]
async fn main() -> std::io::Result<()> {
    env_logger::init();
    info!("starting server");
    let url_map = web::Data::new(UrlMapState {
        url_map: Arc::new(Mutex::new(HashMap::new())),
    });
    HttpServer::new(move || App::new().app_data(url_map.clone()).service(hello).service(redirect))
        .bind(("127.0.0.1", 7777))?
        .run()
        .await
}
