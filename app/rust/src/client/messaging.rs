//! Text messages over the relay mailbox (protocol/README.md §6.2):
//! one sealed copy per recipient device and per own other device, kept in the outbox
//! until that device's E2E receipt arrives (or 24 h pass).

use proto::Id;
use proto::envelope::{self, KIND_MESSAGE, KIND_RECEIPT};
use serde::{Deserialize, Serialize};

use super::Shared;
use super::store::{self, RECEIVED, SENDING};
use crate::config;

/// Most text a message may carry (well under the envelope limit after JSON escaping).
pub const MAX_TEXT: usize = 16 * 1024;

#[derive(Serialize, Deserialize)]
struct MessagePayload {
    /// Logical message id, shared by all copies.
    msg: Id,
    /// Conversation partner as seen by the sender (lets our own devices file the copy).
    to: Id,
    text: String,
}

#[derive(Serialize, Deserialize)]
struct ReceiptPayload {
    msg: Id,
}

/// Writes the message and its copies to the store. The session sends the outbox.
pub fn compose(shared: &Shared, peer_user: &Id, text: &str) -> anyhow::Result<Id> {
    let text = text.trim();
    if text.is_empty() {
        anyhow::bail!("пустое сообщение");
    }
    if text.len() > MAX_TEXT {
        anyhow::bail!("слишком длинное сообщение");
    }
    let msg_id = Id::random();
    let now = config::now();
    let payload = serde_json::to_vec(&MessagePayload { msg: msg_id, to: *peer_user, text: text.to_owned() })?;

    // Every device of the peer, plus our own other devices (they keep the same history).
    let (targets, peer_devices) = {
        let contacts = shared.contacts.lock().unwrap();
        let mut targets: Vec<(Id, [u8; 32])> = vec![];
        let mut peer_devices = 0;
        for c in contacts.all().filter(|c| c.user_id == *peer_user || c.user_id == shared.my_user) {
            for d in &c.devices {
                if d.device_id == shared.me.device_id || shared.manifest.is_revoked(&d.device_id) {
                    continue;
                }
                peer_devices += (c.user_id == *peer_user) as usize;
                targets.push((d.device_id, d.enc_pub));
            }
        }
        (targets, peer_devices)
    };
    if peer_devices == 0 {
        anyhow::bail!("у собеседника нет доступных устройств");
    }

    let store = shared.store.lock().unwrap();
    store.insert_message(&msg_id, peer_user, true, text, now as i64, SENDING)?;
    for (device, enc_pub) in targets {
        let blob = envelope::seal(&shared.me, &device, &enc_pub, KIND_MESSAGE, now, &payload)
            .map_err(|e| anyhow::anyhow!("seal: {e}"))?;
        store.add_outbox(&Id::random(), &msg_id, &device, &blob, now as i64, false)?;
    }
    Ok(msg_id)
}

pub enum Delivered {
    /// Processed (or garbage): ack it. `changed`: conversation to refresh; `new_text`: notify.
    Done { changed: Option<Id>, new_text: Option<(Id, String)> },
    /// Sender not in our contacts yet: keep it on the server, fetch the directory.
    UnknownSender,
}

/// Handles one mailbox delivery. Receipts we owe go to the outbox.
pub fn on_deliver(shared: &Shared, blob: &[u8]) -> anyhow::Result<Delivered> {
    let Ok(unverified) = envelope::open(&shared.me, blob) else {
        return Ok(Delivered::Done { changed: None, new_text: None });
    };
    let sender = unverified.sender();
    let (sender_user, sender_enc, opened) = {
        let contacts = shared.contacts.lock().unwrap();
        let Some((cert, dev)) = contacts.device(&sender, &shared.manifest) else {
            return Ok(Delivered::UnknownSender);
        };
        let (user, enc) = (cert.user_id, dev.enc_pub);
        match unverified.verify(&dev.sign_pub) {
            Ok(o) => (user, enc, o),
            Err(_) => return Ok(Delivered::Done { changed: None, new_text: None }),
        }
    };
    let store = shared.store.lock().unwrap();
    match opened.kind {
        KIND_MESSAGE => {
            let Ok(p) = serde_json::from_slice::<MessagePayload>(&opened.payload) else {
                return Ok(Delivered::Done { changed: None, new_text: None });
            };
            let from_me = sender_user == shared.my_user;
            let peer = if from_me { p.to } else { sender_user };
            let status = if from_me { store::SENT } else { RECEIVED };
            let fresh = store.insert_message(&p.msg, &peer, from_me, &p.text, opened.sent_at as i64, status)?;
            // A receipt even for duplicates: the previous one may have been lost.
            let receipt = serde_json::to_vec(&ReceiptPayload { msg: p.msg })?;
            let now = config::now();
            let rblob = envelope::seal(&shared.me, &sender, &sender_enc, KIND_RECEIPT, now, &receipt)
                .map_err(|e| anyhow::anyhow!("seal: {e}"))?;
            store.add_outbox(&Id::random(), &p.msg, &sender, &rblob, now as i64, true)?;
            let new_text = (fresh && !from_me).then_some((peer, p.text));
            Ok(Delivered::Done { changed: fresh.then_some(peer), new_text })
        }
        KIND_RECEIPT => {
            let Ok(r) = serde_json::from_slice::<ReceiptPayload>(&opened.payload) else {
                return Ok(Delivered::Done { changed: None, new_text: None });
            };
            let changed = store.on_receipt(&r.msg, &sender, sender_user != shared.my_user)?;
            Ok(Delivered::Done { changed, new_text: None })
        }
        _ => Ok(Delivered::Done { changed: None, new_text: None }),
    }
}
