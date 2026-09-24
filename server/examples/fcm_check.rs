//! Checks an FCM service account: obtains an OAuth access token (sends nothing).
//!
//!   cargo run -p relay --example fcm_check -- path/to/service-account.json

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let path = std::env::args().nth(1).expect("usage: fcm_check <service-account.json>");
    let p = relay::fcm::FcmPusher::from_json(&std::fs::read(path).expect("read")).expect("parse");
    match p.check().await {
        Ok(()) => println!("ok: service account can get FCM access tokens"),
        Err(e) => {
            println!("failed: {e}");
            std::process::exit(1);
        }
    }
}
