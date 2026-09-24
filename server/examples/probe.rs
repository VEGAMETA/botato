//! Diagnostic: connect to a relay, run the first half of the handshake and check the
//! server's proof of key possession. Reveals no identity (no cert is sent).
//!
//!   cargo run -p relay --example probe -- wss://relay.example.org/ws <server_pub hex>

use futures_util::{SinkExt, StreamExt};
use proto::auth::{self, Nonce};
use proto::frames::{ClientFrame, ServerFrame};
use tokio_tungstenite::tungstenite::Message;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (url, want) = match args.as_slice() {
        [_, url, want] => (url.clone(), want.to_lowercase()),
        _ => {
            eprintln!("usage: probe <wss-url> <server_pub hex>");
            std::process::exit(2);
        }
    };
    let _ = rustls::crypto::ring::default_provider().install_default();
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.expect("connect");
    let cn: Nonce = proto::random_bytes();
    let hello = ClientFrame::Hello { v: 1, nonce: cn.to_vec() };
    ws.send(Message::Text(serde_json::to_string(&hello).unwrap().into())).await.expect("send");
    let msg = ws.next().await.expect("eof").expect("read");
    let Message::Text(t) = msg else { panic!("unexpected {msg:?}") };
    let ServerFrame::Challenge { nonce, server_pub, sig } = serde_json::from_str(t.as_str()).expect("frame") else {
        panic!("not a challenge: {t}")
    };
    let sn: Nonce = nonce.try_into().expect("nonce length");
    let got = hex::encode(&server_pub);
    let proof = auth::verify_server(&server_pub, &cn, &sn, &sig).is_ok();
    println!("server_pub {got}");
    println!("proof      {}", if proof { "valid" } else { "INVALID" });
    println!("expected   {}", if got == want { "match" } else { "MISMATCH" });
    if !(proof && got == want) {
        std::process::exit(1);
    }
}
