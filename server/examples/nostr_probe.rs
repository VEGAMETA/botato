//! Diagnostic for the Nostr fallback (protocol/README.md §7): which public relays are reachable
//! from here and actually forward our ephemeral signal events. For each relay: subscribe to a
//! random tag, publish an event with it on a second connection, wait for it to come back.
//!
//!   cargo run -p relay --example nostr_probe -- wss://nos.lol wss://relay.damus.io ...

use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;

async fn probe(url: &str) -> Result<Duration, String> {
    let t = Duration::from_secs(8);
    let (mut sub, _) = timeout(t, tokio_tungstenite::connect_async(url)).await.map_err(|_| "connect timeout")?.map_err(|e| format!("connect: {e}"))?;
    let (mut publ, _) = timeout(t, tokio_tungstenite::connect_async(url)).await.map_err(|_| "connect timeout")?.map_err(|e| format!("connect: {e}"))?;
    let tag = hex::encode(proto::random_bytes::<16>());
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let req = json!(["REQ", "probe", {"kinds": [proto::nostr::KIND_SIGNAL], "#t": [tag], "since": now - 60}]);
    sub.send(Message::Text(req.to_string().into())).await.map_err(|e| format!("req: {e}"))?;
    // Let the subscription settle (EOSE) before publishing: ephemeral events aren't replayed.
    let _ = timeout(Duration::from_secs(3), async {
        while let Some(Ok(Message::Text(m))) = sub.next().await {
            if m.as_str().starts_with("[\"EOSE\"") {
                break;
            }
        }
    })
    .await;
    let ev = proto::nostr::signal_event(b"botato probe", &tag, now).map_err(|e| format!("sign: {e}"))?;
    let start = Instant::now();
    publ.send(Message::Text(json!(["EVENT", ev.to_json()]).to_string().into())).await.map_err(|e| format!("publish: {e}"))?;
    let got = timeout(t, async {
        loop {
            tokio::select! {
                m = sub.next() => match m {
                    Some(Ok(Message::Text(m))) => {
                        let v: Value = serde_json::from_str(m.as_str()).unwrap_or(Value::Null);
                        if v[0] == "EVENT" && v[2]["id"] == ev.id {
                            return Ok(start.elapsed());
                        }
                    }
                    Some(Ok(_)) => {}
                    other => return Err(format!("subscription closed: {other:?}")),
                },
                m = publ.next() => if let Some(Ok(Message::Text(m))) = m {
                    let v: Value = serde_json::from_str(m.as_str()).unwrap_or(Value::Null);
                    if v[0] == "OK" && v[2] == false {
                        return Err(format!("rejected: {}", v[3]));
                    }
                },
            }
        }
    })
    .await;
    got.map_err(|_| "event did not come back".to_string())?
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let urls: Vec<String> = std::env::args().skip(1).collect();
    if urls.is_empty() {
        eprintln!("usage: nostr_probe <wss-url>...");
        std::process::exit(2);
    }
    let results = futures_util::future::join_all(urls.iter().map(|u| async move { (u, probe(u).await) })).await;
    for (u, r) in results {
        match r {
            Ok(d) => println!("OK    {u}  {} ms", d.as_millis()),
            Err(e) => println!("FAIL  {u}  {e}"),
        }
    }
}
