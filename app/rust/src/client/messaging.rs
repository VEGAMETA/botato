//! Messages over the relay mailbox (protocol/README.md §6.2, photos §6.3):
//! one sealed copy per recipient device and per own other device, kept in the outbox
//! until that device's E2E receipt arrives (or 24 h pass).

use proto::Id;
use proto::envelope::{self, KIND_MESSAGE, KIND_RECEIPT};
use serde::{Deserialize, Serialize};

use super::store::{self, PhotoMeta, RECEIVED, SENDING};
use super::{Shared, photo};
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    photo: Option<PhotoPayload>,
}

/// §6.3: where to fetch the photo and how to open it.
#[derive(Serialize, Deserialize)]
struct PhotoPayload {
    file: Id,
    #[serde(with = "b64")]
    key: Vec<u8>,
    size: u32,
    w: u32,
    h: u32,
    #[serde(with = "b64")]
    thumb: Vec<u8>,
}

impl PhotoPayload {
    fn meta(&self) -> Option<PhotoMeta> {
        let key: [u8; 32] = self.key.as_slice().try_into().ok()?;
        let ok = self.size as usize <= proto::file::MAX_FILE
            && self.thumb.len() <= photo::MAX_THUMB
            && (1..=photo::MAX_SIDE * 4).contains(&self.w)
            && (1..=photo::MAX_SIDE * 4).contains(&self.h);
        ok.then(|| PhotoMeta { file_id: self.file, key, size: self.size, w: self.w, h: self.h, thumb: self.thumb.clone() })
    }
}

mod b64 {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(v: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&STANDARD.encode(v))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        STANDARD.decode(String::deserialize(d)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Serialize, Deserialize)]
struct ReceiptPayload {
    msg: Id,
}

/// Notification / conversation-list text for a message.
pub fn preview(text: &str, photo: bool) -> String {
    match (photo, text.is_empty()) {
        (false, _) => text.to_owned(),
        (true, true) => "📷 Фото".to_owned(),
        (true, false) => format!("📷 {text}"),
    }
}

/// Writes the message and its copies to the store. The session sends the outbox.
pub fn compose(shared: &Shared, peer_user: &Id, text: &str) -> anyhow::Result<Id> {
    if text.trim().is_empty() {
        anyhow::bail!("пустое сообщение");
    }
    compose_with(shared, peer_user, text, None)
}

/// Re-encodes the picture, then queues the message; the session uploads the file first.
pub fn compose_photo(shared: &Shared, peer_user: &Id, image: &[u8], caption: &str) -> anyhow::Result<Id> {
    let p = photo::prepare(image)?;
    let key: [u8; 32] = proto::random_bytes();
    let file_id = Id::random();
    let size = proto::file::seal(&key, &file_id, &p.jpeg).map_err(|e| anyhow::anyhow!("{e}"))?.len() as u32;
    let meta = PhotoMeta { file_id, key, size, w: p.w, h: p.h, thumb: p.thumb };
    compose_with(shared, peer_user, caption, Some((meta, p.jpeg)))
}

fn compose_with(shared: &Shared, peer_user: &Id, text: &str, photo: Option<(PhotoMeta, Vec<u8>)>) -> anyhow::Result<Id> {
    let text = text.trim();
    if text.len() > MAX_TEXT {
        anyhow::bail!("слишком длинное сообщение");
    }
    let msg_id = Id::random();
    let now = config::now();
    let photo_payload = photo.as_ref().map(|(m, _)| PhotoPayload {
        file: m.file_id,
        key: m.key.to_vec(),
        size: m.size,
        w: m.w,
        h: m.h,
        thumb: m.thumb.clone(),
    });
    let payload =
        serde_json::to_vec(&MessagePayload { msg: msg_id, to: *peer_user, text: text.to_owned(), photo: photo_payload })?;

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
    if let Some((meta, jpeg)) = &photo {
        store.insert_photo(&msg_id, meta, Some(jpeg))?;
    }
    for (device, enc_pub) in targets {
        let blob = envelope::seal(&shared.me, &device, &enc_pub, KIND_MESSAGE, now, &payload)
            .map_err(|e| anyhow::anyhow!("seal: {e}"))?;
        store.add_outbox(&Id::random(), &msg_id, &device, &blob, now as i64, false)?;
    }
    if let Some((meta, _)) = &photo {
        store.add_upload(&meta.file_id, &msg_id)?;
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
            let meta = match &p.photo {
                None => None,
                Some(ph) => match ph.meta() {
                    Some(m) => Some(m),
                    None => return Ok(Delivered::Done { changed: None, new_text: None }), // malformed
                },
            };
            let fresh = store.insert_message(&p.msg, &peer, from_me, &p.text, opened.sent_at as i64, status)?;
            if let Some(m) = &meta {
                store.insert_photo(&p.msg, m, None)?;
            }
            // A receipt even for duplicates: the previous one may have been lost.
            let receipt = serde_json::to_vec(&ReceiptPayload { msg: p.msg })?;
            let now = config::now();
            let rblob = envelope::seal(&shared.me, &sender, &sender_enc, KIND_RECEIPT, now, &receipt)
                .map_err(|e| anyhow::anyhow!("seal: {e}"))?;
            store.add_outbox(&Id::random(), &p.msg, &sender, &rblob, now as i64, true)?;
            let new_text = (fresh && !from_me).then(|| (peer, preview(&p.text, meta.is_some())));
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

/// A downloaded attachment: checks and stores it. Returns the conversation to refresh.
pub fn on_file(shared: &Shared, file_id: &Id, blob: &[u8]) -> anyhow::Result<Option<Id>> {
    let store = shared.store.lock().unwrap();
    let Some((msg_id, meta, _, peer)) = store.photo_by_file(file_id)? else { return Ok(None) };
    let opened = (blob.len() == meta.size as usize)
        .then(|| proto::file::open(&meta.key, file_id, blob).ok())
        .flatten();
    match opened {
        Some(jpeg) => store.set_photo_data(&msg_id, &jpeg)?,
        // The relay served something else: it will not get better by asking again.
        None => store.mark_photo_gone(&msg_id)?,
    }
    Ok(Some(peer))
}

/// The relay has no such file. Only final once the 24 h a relay keeps files are over;
/// before that the sender may still re-upload it (after a relay restart).
pub fn on_file_missing(shared: &Shared, file_id: &Id) -> anyhow::Result<Option<Id>> {
    let store = shared.store.lock().unwrap();
    let Some((msg_id, _, sent_at, peer)) = store.photo_by_file(file_id)? else { return Ok(None) };
    if config::now() as i64 - sent_at < store::OUTBOX_TTL_SECS {
        return Ok(None);
    }
    store.mark_photo_gone(&msg_id)?;
    Ok(Some(peer))
}
