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

/// The manifest event (kind 30078) must be stored: publish one under a random `d`, read it back.
async fn probe_stored(url: &str) -> Result<(), String> {
    let t = Duration::from_secs(8);
    let (mut ws, _) = timeout(t, tokio_tungstenite::connect_async(url)).await.map_err(|_| "connect timeout")?.map_err(|e| format!("connect: {e}"))?;
    let root = proto::random_bytes::<32>();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let ev = proto::nostr::manifest_event(&proto::nostr::random_secp_key(), b"probe", &root, now).map_err(|e| e.to_string())?;
    ws.send(Message::Text(json!(["EVENT", ev.to_json()]).to_string().into())).await.map_err(|e| e.to_string())?;
    let ok = timeout(t, async {
        while let Some(Ok(Message::Text(m))) = ws.next().await {
            let v: Value = serde_json::from_str(m.as_str()).unwrap_or(Value::Null);
            if v[0] == "OK" {
                return if v[2] == true { Ok(()) } else { Err(format!("rejected: {}", v[3])) };
            }
        }
        Err("closed".to_string())
    })
    .await
    .map_err(|_| "no OK".to_string())?;
    ok?;
    let d = proto::nostr::manifest_d_tag(&root);
    ws.send(Message::Text(json!(["REQ", "m", {"kinds": [proto::nostr::KIND_MANIFEST], "#d": [d]}]).to_string().into())).await.map_err(|e| e.to_string())?;
    timeout(t, async {
        while let Some(Ok(Message::Text(m))) = ws.next().await {
            let v: Value = serde_json::from_str(m.as_str()).unwrap_or(Value::Null);
            if v[0] == "EVENT" && v[2]["id"] == ev.id {
                return Ok(());
            }
            if v[0] == "EOSE" {
                return Err("not stored".to_string());
            }
        }
        Err("closed".to_string())
    })
    .await
    .map_err(|_| "no answer".to_string())?
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let urls: Vec<String> = std::env::args().skip(1).collect();
    if urls.is_empty() {
        eprintln!("usage: nostr_probe <wss-url>...");
        std::process::exit(2);
    }
    let results = futures_util::future::join_all(urls.iter().map(|u| async move { (u, probe(u).await, probe_stored(u).await) })).await;
    for (u, signal, stored) in results {
        let signal = match signal {
            Ok(d) => format!("signal ok {} ms", d.as_millis()),
            Err(e) => format!("signal FAIL ({e})"),
        };
        let stored = match stored {
            Ok(()) => "manifest ok".to_string(),
            Err(e) => format!("manifest FAIL ({e})"),
        };
        println!("{u}: {signal}; {stored}");
    }
}
