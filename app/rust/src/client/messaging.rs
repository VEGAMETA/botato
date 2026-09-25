//! Messages over the relay mailbox (protocol/README.md §6.2, photos §6.3):
//! one sealed copy per recipient device and per own other device, kept in the outbox
//! until that device's E2E receipt arrives (or 24 h pass).

use proto::Id;
use proto::envelope::{self, KIND_MESSAGE, KIND_RECEIPT, KIND_UPDATE};
use serde::{Deserialize, Serialize};

use super::store::{self, Extra, PhotoMeta, RECEIVED, ReplyRef, SENDING};
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    voice: Option<VoicePayload>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reply: Option<ReplyRef>,
    /// Display name of the original author of a forwarded message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    fwd: Option<String>,
}

/// Kind 4: edit, delete for everyone, or reaction on `msg` (§6.2).
#[derive(Serialize, Deserialize)]
struct UpdatePayload {
    /// Logical id of this update (outbox, receipts).
    id: Id,
    msg: Id,
    to: Id,
    op: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    emoji: Option<String>,
}

/// Quote snapshot length in a reply.
const MAX_QUOTE: usize = 200;
/// A reaction is one emoji (possibly a multi-codepoint sequence).
const MAX_EMOJI: usize = 32;

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
        ok.then(|| PhotoMeta {
            file_id: self.file,
            key,
            size: self.size,
            w: self.w,
            h: self.h,
            thumb: self.thumb.clone(),
            kind: store::KIND_PHOTO,
        })
    }
}

/// §6.3: a voice message (AAC in MP4/M4A), fetched and opened like a photo.
#[derive(Serialize, Deserialize)]
struct VoicePayload {
    file: Id,
    #[serde(with = "b64")]
    key: Vec<u8>,
    size: u32,
    /// Duration, milliseconds.
    ms: u32,
    /// Loudness bars for the bubble, 0..=255 each.
    #[serde(with = "b64")]
    wave: Vec<u8>,
}

/// Longest voice message, ms.
pub const MAX_VOICE_MS: u32 = 10 * 60 * 1000;
/// Waveform bars carried in the message.
pub const MAX_WAVE: usize = 128;

impl VoicePayload {
    fn meta(&self) -> Option<PhotoMeta> {
        let key: [u8; 32] = self.key.as_slice().try_into().ok()?;
        let ok = self.size as usize <= proto::file::MAX_FILE
            && (1..=MAX_VOICE_MS).contains(&self.ms)
            && self.wave.len() <= MAX_WAVE;
        ok.then(|| PhotoMeta {
            file_id: self.file,
            key,
            size: self.size,
            w: self.ms,
            h: 0,
            thumb: self.wave.clone(),
            kind: store::KIND_VOICE,
        })
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

/// Notification / conversation-list text for a message with an attachment of `attach` kind.
pub fn preview(text: &str, attach: Option<u8>) -> String {
    let label = match attach {
        None => return text.to_owned(),
        Some(store::KIND_VOICE) => "🎤 Голосовое",
        Some(_) => "📷 Фото",
    };
    if text.is_empty() { label.to_owned() } else { format!("{} {text}", label.split(' ').next().unwrap_or("")) }
}

/// Writes the message and its copies to the store. The session sends the outbox.
pub fn compose(shared: &Shared, peer_user: &Id, text: &str, reply: Option<&Id>) -> anyhow::Result<Id> {
    if text.trim().is_empty() {
        anyhow::bail!("пустое сообщение");
    }
    let extra = Extra { reply: reply.map(|r| reply_ref(shared, r)).transpose()?, fwd: None };
    compose_with(shared, peer_user, text, None, extra)
}

/// Re-encodes the picture, then queues the message; the session uploads the file first.
/// `keep_png`: a PNG may be sent as PNG (screenshots); see `photo::prepare`.
pub fn compose_photo(
    shared: &Shared,
    peer_user: &Id,
    image: &[u8],
    caption: &str,
    reply: Option<&Id>,
    keep_png: bool,
) -> anyhow::Result<Id> {
    let p = photo::prepare(image, keep_png)?;
    let extra = Extra { reply: reply.map(|r| reply_ref(shared, r)).transpose()?, fwd: None };
    compose_with(shared, peer_user, caption, Some(new_attachment(store::KIND_PHOTO, p.data, p.thumb, p.w, p.h)?), extra)
}

/// Queues a recorded voice message (AAC in M4A as the recorder wrote it; sent as is).
pub fn compose_voice(
    shared: &Shared,
    peer_user: &Id,
    audio: Vec<u8>,
    ms: u32,
    wave: Vec<u8>,
    reply: Option<&Id>,
) -> anyhow::Result<Id> {
    if audio.is_empty() || audio.len() > proto::file::MAX_PLAINTEXT {
        anyhow::bail!("голосовое слишком длинное");
    }
    if !(1..=MAX_VOICE_MS).contains(&ms) {
        anyhow::bail!("голосовое слишком длинное");
    }
    let wave: Vec<u8> = wave.into_iter().take(MAX_WAVE).collect();
    let extra = Extra { reply: reply.map(|r| reply_ref(shared, r)).transpose()?, fwd: None };
    compose_with(shared, peer_user, "", Some(new_attachment(store::KIND_VOICE, audio, wave, ms, 0)?), extra)
}

/// Sends a copy of a stored message to `to_peer`, marked as forwarded from `from_name`.
/// A photo goes up again under a new file id and key.
pub fn forward(shared: &Shared, msg_id: &Id, to_peer: &Id, from_name: &str) -> anyhow::Result<Id> {
    let original = shared.store.lock().unwrap().original(msg_id)?;
    let Some(o) = original.filter(|o| !o.deleted) else { anyhow::bail!("сообщения нет") };
    let photo = match o.photo {
        None => None,
        Some((_, None)) => anyhow::bail!("фото ещё не загрузилось"),
        Some((meta, Some(data))) => Some(new_attachment(meta.kind, data, meta.thumb, meta.w, meta.h)?),
    };
    if photo.is_none() && o.text.trim().is_empty() {
        anyhow::bail!("нечего пересылать");
    }
    let fwd: String = from_name.chars().take(64).collect();
    compose_with(shared, to_peer, &o.text, photo, Extra { reply: None, fwd: Some(fwd) })
}

/// A fresh file id and key for an attachment (photo or voice) about to be uploaded.
fn new_attachment(kind: u8, data: Vec<u8>, thumb: Vec<u8>, w: u32, h: u32) -> anyhow::Result<(PhotoMeta, Vec<u8>)> {
    let key: [u8; 32] = proto::random_bytes();
    let file_id = Id::random();
    let size = proto::file::seal(&key, &file_id, &data).map_err(|e| anyhow::anyhow!("{e}"))?.len() as u32;
    Ok((PhotoMeta { file_id, key, size, w, h, thumb, kind }, data))
}

/// Snapshot of the quoted message (§6.2 Reply).
fn reply_ref(shared: &Shared, msg: &Id) -> anyhow::Result<ReplyRef> {
    let original = shared.store.lock().unwrap().original(msg)?;
    let Some(o) = original else { anyhow::bail!("сообщения нет") };
    let text = preview(&o.text, o.photo.as_ref().map(|(m, _)| m.kind));
    Ok(ReplyRef {
        msg: *msg,
        from: if o.from_me { shared.my_user } else { o.peer },
        text: text.chars().take(MAX_QUOTE).collect(),
    })
}

/// Every device of the peer, plus our own other devices (they keep the same history).
fn targets(shared: &Shared, peer_user: &Id) -> anyhow::Result<Vec<(Id, [u8; 32])>> {
    let contacts = shared.contacts.lock().unwrap();
    let mut targets: Vec<(Id, [u8; 32])> = vec![];
    let mut peer_devices = 0;
    for c in contacts.all().filter(|c| c.user_id == *peer_user || c.user_id == shared.my_user) {
        for d in &c.devices {
            if d.device_id == shared.me.device_id || shared.manifest().is_revoked(&d.device_id) {
                continue;
            }
            peer_devices += (c.user_id == *peer_user) as usize;
            targets.push((d.device_id, d.enc_pub));
        }
    }
    if peer_devices == 0 {
        anyhow::bail!("у собеседника нет доступных устройств");
    }
    Ok(targets)
}

/// Seals `payload` for every target and puts the copies in the outbox under `logical_id`.
fn enqueue(
    shared: &Shared,
    store: &store::Store,
    targets: Vec<(Id, [u8; 32])>,
    kind: u8,
    logical_id: &Id,
    payload: &[u8],
    now: u64,
) -> anyhow::Result<()> {
    for (device, enc_pub) in targets {
        let blob = envelope::seal(&shared.me, &device, &enc_pub, kind, now, payload).map_err(|e| anyhow::anyhow!("seal: {e}"))?;
        store.add_outbox(&Id::random(), logical_id, &device, &blob, now as i64, false)?;
    }
    Ok(())
}

fn compose_with(
    shared: &Shared,
    peer_user: &Id,
    text: &str,
    photo: Option<(PhotoMeta, Vec<u8>)>,
    extra: Extra,
) -> anyhow::Result<Id> {
    let text = text.trim();
    if text.len() > MAX_TEXT {
        anyhow::bail!("слишком длинное сообщение");
    }
    let msg_id = Id::random();
    let now = config::now();
    let photo_payload = photo.as_ref().filter(|(m, _)| m.kind == store::KIND_PHOTO).map(|(m, _)| PhotoPayload {
        file: m.file_id,
        key: m.key.to_vec(),
        size: m.size,
        w: m.w,
        h: m.h,
        thumb: m.thumb.clone(),
    });
    let voice_payload = photo.as_ref().filter(|(m, _)| m.kind == store::KIND_VOICE).map(|(m, _)| VoicePayload {
        file: m.file_id,
        key: m.key.to_vec(),
        size: m.size,
        ms: m.w,
        wave: m.thumb.clone(),
    });
    let payload = serde_json::to_vec(&MessagePayload {
        msg: msg_id,
        to: *peer_user,
        text: text.to_owned(),
        photo: photo_payload,
        voice: voice_payload,
        reply: extra.reply.clone(),
        fwd: extra.fwd.clone(),
    })?;
    let targets = targets(shared, peer_user)?;

    let store = shared.store.lock().unwrap();
    store.insert_message(&msg_id, peer_user, true, text, now as i64, SENDING)?;
    if extra != Extra::default() {
        store.set_extra(&msg_id, &extra)?;
    }
    if let Some((meta, jpeg)) = &photo {
        store.insert_photo(&msg_id, meta, Some(jpeg))?;
    }
    enqueue(shared, &store, targets, KIND_MESSAGE, &msg_id, &payload, now)?;
    if let Some((meta, _)) = &photo {
        store.add_upload(&meta.file_id, &msg_id)?;
    }
    Ok(msg_id)
}

/// What a kind-4 update does (§6.2).
pub enum Update {
    Edit(String),
    Delete,
    React(String),
}

/// Applies an edit, delete-for-everyone or reaction here and sends it to the conversation.
pub fn update(shared: &Shared, peer_user: &Id, msg: &Id, op: Update) -> anyhow::Result<()> {
    let now = config::now();
    let (op_name, text, emoji) = match &op {
        Update::Edit(t) => {
            let t = t.trim();
            if t.is_empty() {
                anyhow::bail!("пустое сообщение");
            }
            if t.len() > MAX_TEXT {
                anyhow::bail!("слишком длинное сообщение");
            }
            ("edit", Some(t.to_owned()), None)
        }
        Update::Delete => ("delete", None, None),
        Update::React(e) => {
            if e.len() > MAX_EMOJI {
                anyhow::bail!("слишком длинная реакция");
            }
            ("react", None, Some(e.clone()))
        }
    };
    let id = Id::random();
    let payload = serde_json::to_vec(&UpdatePayload { id, msg: *msg, to: *peer_user, op: op_name.into(), text, emoji })?;
    let targets = targets(shared, peer_user)?;
    let store = shared.store.lock().unwrap();
    let me = shared.my_user;
    let applied = match op {
        Update::Edit(t) => store.apply_edit(msg, &me, &me, t.trim(), now as i64)?,
        Update::Delete => store.apply_delete(msg, &me, &me)?,
        Update::React(e) => store.apply_react(msg, &me, &me, &e, now as i64)?,
    };
    if applied.is_none() {
        anyhow::bail!("это сообщение нельзя изменить");
    }
    enqueue(shared, &store, targets, KIND_UPDATE, &id, &payload, now)
}

pub enum Delivered {
    /// Processed (or garbage): ack it. `changed`: conversation to refresh; `new_text`: notify.
    Done { changed: Option<Id>, new_text: Option<(Id, String)> },
    /// Sender not in our contacts yet: keep it on the server, fetch the directory.
    UnknownSender,
}

const DROP: Delivered = Delivered::Done { changed: None, new_text: None };

/// Handles one mailbox delivery. Receipts we owe go to the outbox.
pub fn on_deliver(shared: &Shared, blob: &[u8]) -> anyhow::Result<Delivered> {
    let Ok(unverified) = envelope::open(&shared.me, blob) else {
        return Ok(DROP);
    };
    let sender = unverified.sender();
    let (sender_user, sender_enc, opened) = {
        let contacts = shared.contacts.lock().unwrap();
        let Some((cert, dev)) = contacts.device(&sender, &shared.manifest()) else {
            return Ok(Delivered::UnknownSender);
        };
        let (user, enc) = (cert.user_id, dev.enc_pub);
        match unverified.verify(&dev.sign_pub) {
            Ok(o) => (user, enc, o),
            Err(_) => return Ok(DROP),
        }
    };
    let store = shared.store.lock().unwrap();
    // A receipt for every copy, duplicates too: the previous one may have been lost.
    let receipt = |logical: &Id| -> anyhow::Result<()> {
        let body = serde_json::to_vec(&ReceiptPayload { msg: *logical })?;
        let now = config::now();
        let rblob = envelope::seal(&shared.me, &sender, &sender_enc, KIND_RECEIPT, now, &body)
            .map_err(|e| anyhow::anyhow!("seal: {e}"))?;
        store.add_outbox(&Id::random(), logical, &sender, &rblob, now as i64, true)
    };
    match opened.kind {
        KIND_MESSAGE => {
            let Ok(p) = serde_json::from_slice::<MessagePayload>(&opened.payload) else {
                return Ok(DROP);
            };
            let from_me = sender_user == shared.my_user;
            let peer = if from_me { p.to } else { sender_user };
            let status = if from_me { store::SENT } else { RECEIVED };
            let meta = match (&p.photo, &p.voice) {
                (None, None) => None,
                (Some(ph), None) => Some(ph.meta().ok_or(()).map_err(|_| ())),
                (None, Some(v)) => Some(v.meta().ok_or(()).map_err(|_| ())),
                (Some(_), Some(_)) => return Ok(DROP), // one attachment per message
            };
            let meta = match meta {
                None => None,
                Some(Ok(m)) => Some(m),
                Some(Err(())) => return Ok(DROP), // malformed
            };
            let fresh = store.insert_message(&p.msg, &peer, from_me, &p.text, opened.sent_at as i64, status)?;
            if fresh {
                let extra = Extra {
                    reply: p.reply.map(|mut r| {
                        r.text = r.text.chars().take(MAX_QUOTE).collect();
                        r
                    }),
                    fwd: p.fwd.map(|f| f.chars().take(64).collect()),
                };
                if extra != Extra::default() {
                    store.set_extra(&p.msg, &extra)?;
                }
            }
            if let Some(m) = &meta {
                store.insert_photo(&p.msg, m, None)?;
            }
            receipt(&p.msg)?;
            let new_text = (fresh && !from_me).then(|| (peer, preview(&p.text, meta.as_ref().map(|m| m.kind))));
            Ok(Delivered::Done { changed: fresh.then_some(peer), new_text })
        }
        KIND_UPDATE => {
            let Ok(u) = serde_json::from_slice::<UpdatePayload>(&opened.payload) else {
                return Ok(DROP);
            };
            let at = opened.sent_at as i64;
            let me = shared.my_user;
            let changed = match (u.op.as_str(), u.text, u.emoji) {
                ("edit", Some(t), _) if !t.trim().is_empty() && t.len() <= MAX_TEXT => {
                    store.apply_edit(&u.msg, &sender_user, &me, t.trim(), at)?
                }
                ("delete", _, _) => store.apply_delete(&u.msg, &sender_user, &me)?,
                ("react", _, Some(e)) if e.len() <= MAX_EMOJI => store.apply_react(&u.msg, &sender_user, &me, &e, at)?,
                _ => None,
            };
            receipt(&u.id)?;
            Ok(Delivered::Done { changed, new_text: None })
        }
        KIND_RECEIPT => {
            let Ok(r) = serde_json::from_slice::<ReceiptPayload>(&opened.payload) else {
                return Ok(DROP);
            };
            let changed = store.on_receipt(&r.msg, &sender, sender_user != shared.my_user)?;
            Ok(Delivered::Done { changed, new_text: None })
        }
        _ => Ok(DROP),
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
