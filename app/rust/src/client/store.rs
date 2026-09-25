//! Local history and outbox (SQLite). Message bodies are encrypted with a key kept in
//! platform secure storage; metadata (who, when) stays plain inside the local file.

use std::path::Path;

use chacha20poly1305::aead::array::Array;
use chacha20poly1305::aead::{Aead, Payload};
use chacha20poly1305::{KeyInit, XChaCha20Poly1305, XNonce};
use proto::Id;
use rusqlite::{Connection, OptionalExtension, params};

/// Outgoing message states.
pub const SENDING: i64 = 0; // in the outbox, not yet accepted by a relay
pub const SENT: i64 = 1; // a relay accepted at least one copy
pub const DELIVERED: i64 = 2; // E2E receipt from a device of the recipient
pub const FAILED: i64 = 3; // no receipt within 24 h
pub const RECEIVED: i64 = 10;

/// An undelivered copy is dropped this long after the message was created.
pub const OUTBOX_TTL_SECS: i64 = 24 * 3600;

pub struct Store {
    db: Connection,
    key: [u8; 32],
}

pub struct Message {
    pub msg_id: Id,
    pub from_me: bool,
    pub text: String,
    pub sent_at: i64,
    pub status: i64,
    pub photo: Option<PhotoView>,
    pub edited: bool,
    /// Deleted for everyone: `text` is empty, the bubble says so.
    pub deleted: bool,
    pub extra: Extra,
    /// (user, emoji), one per user.
    pub reactions: Vec<(Id, String)>,
}

/// Reply and forward info of a message (protocol/README.md §6.2), kept encrypted.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Extra {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply: Option<ReplyRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fwd: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ReplyRef {
    pub msg: Id,
    pub from: Id,
    pub text: String,
}

/// What a message is, for quoting and forwarding.
pub struct Original {
    pub peer: Id,
    pub from_me: bool,
    pub text: String,
    pub deleted: bool,
    pub photo: Option<(PhotoMeta, Option<Vec<u8>>)>,
}

/// What the E2E message says about an attached photo (protocol/README.md §6.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PhotoMeta {
    pub file_id: Id,
    pub key: [u8; 32],
    pub size: u32,
    pub w: u32,
    pub h: u32,
    pub thumb: Vec<u8>,
}

pub struct PhotoView {
    pub w: u32,
    pub h: u32,
    pub thumb: Vec<u8>,
    /// The full photo is stored locally.
    pub ready: bool,
    /// The relay no longer has it and we never got it.
    pub gone: bool,
}

pub struct Upload {
    pub file_id: Id,
    pub key: [u8; 32],
    pub data: Vec<u8>,
}

pub struct Conversation {
    pub peer_user: Id,
    pub last_text: String,
    pub last_photo: bool,
    pub last_deleted: bool,
    pub last_at: i64,
    pub last_from_me: bool,
    pub unread: i64,
}

pub struct OutboxRow {
    pub id: Id,
    pub to_device: Id,
    pub blob: Vec<u8>,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS messages (
    msg_id      BLOB PRIMARY KEY,
    peer_user   BLOB NOT NULL,
    from_me     INTEGER NOT NULL,
    sent_at     INTEGER NOT NULL,
    body        BLOB NOT NULL,
    status      INTEGER NOT NULL,
    read        INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS messages_peer ON messages (peer_user, sent_at);
CREATE TABLE IF NOT EXISTS outbox (
    id          BLOB PRIMARY KEY,
    msg_id      BLOB NOT NULL,
    to_device   BLOB NOT NULL,
    blob        BLOB NOT NULL,
    created_at  INTEGER NOT NULL,
    receipt     INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS outbox_msg ON outbox (msg_id);
CREATE TABLE IF NOT EXISTS photos (
    msg_id      BLOB PRIMARY KEY,
    file_id     BLOB NOT NULL,
    meta        BLOB NOT NULL,
    data        BLOB,
    gone        INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS photos_file ON photos (file_id);
CREATE TABLE IF NOT EXISTS uploads (
    file_id     BLOB PRIMARY KEY,
    msg_id      BLOB NOT NULL
);
CREATE TABLE IF NOT EXISTS reactions (
    msg_id      BLOB NOT NULL,
    user_id     BLOB NOT NULL,
    emoji       BLOB NOT NULL,
    at          INTEGER NOT NULL,
    PRIMARY KEY (msg_id, user_id)
);
";

/// Columns added after the first release; `ALTER TABLE` keeps existing history.
fn migrate(db: &Connection) -> anyhow::Result<()> {
    let cols: Vec<String> = db
        .prepare("PRAGMA table_info(messages)")?
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<Result<_, _>>()?;
    for (name, ddl) in [("edited_at", "INTEGER"), ("deleted", "INTEGER NOT NULL DEFAULT 0"), ("extra", "BLOB")] {
        if !cols.iter().any(|c| c == name) {
            db.execute(&format!("ALTER TABLE messages ADD COLUMN {name} {ddl}"), [])?;
        }
    }
    // 0.1.3 kept "deleted" placeholders; deletions now leave nothing.
    db.execute("DELETE FROM messages WHERE deleted = 1", [])?;
    Ok(())
}

fn id(b: Vec<u8>) -> Id {
    Id(b.try_into().unwrap_or([0; 16]))
}

impl Store {
    pub fn open(path: &Path, key: [u8; 32]) -> anyhow::Result<Store> {
        let db = Connection::open(path)?;
        db.pragma_update(None, "journal_mode", "WAL")?;
        db.pragma_update(None, "secure_delete", "ON")?;
        db.execute_batch(SCHEMA)?;
        migrate(&db)?;
        Ok(Store { db, key })
    }

    fn seal(&self, msg_id: &Id, text: &str) -> Vec<u8> {
        self.seal_bytes(&msg_id.0, text.as_bytes())
    }

    fn unseal(&self, msg_id: &Id, body: &[u8]) -> String {
        if body.len() < 24 {
            return String::new();
        }
        self.unseal_bytes(&msg_id.0, body)
            .and_then(|p| String::from_utf8(p).ok())
            .unwrap_or_else(|| "⚠ не удалось расшифровать".into())
    }

    /// `aad` ties the ciphertext to its row and column.
    fn seal_bytes(&self, aad: &[u8], data: &[u8]) -> Vec<u8> {
        let nonce: [u8; 24] = proto::random_bytes();
        let ct = XChaCha20Poly1305::new(&Array::from(self.key))
            .encrypt(&XNonce::from(nonce), Payload { msg: data, aad })
            .expect("encryption cannot fail");
        [&nonce[..], &ct].concat()
    }

    fn unseal_bytes(&self, aad: &[u8], body: &[u8]) -> Option<Vec<u8>> {
        if body.len() < 24 {
            return None;
        }
        let (nonce, ct) = body.split_at(24);
        XChaCha20Poly1305::new(&Array::from(self.key))
            .decrypt(&XNonce::from(<[u8; 24]>::try_from(nonce).unwrap()), Payload { msg: ct, aad })
            .ok()
    }

    fn photo_aad(msg_id: &Id, what: &[u8]) -> Vec<u8> {
        [&msg_id.0[..], what].concat()
    }

    fn seal_meta(&self, msg_id: &Id, m: &PhotoMeta) -> Vec<u8> {
        let mut b = Vec::with_capacity(44 + m.thumb.len());
        b.extend_from_slice(&m.key);
        b.extend_from_slice(&m.size.to_be_bytes());
        b.extend_from_slice(&m.w.to_be_bytes());
        b.extend_from_slice(&m.h.to_be_bytes());
        b.extend_from_slice(&m.thumb);
        self.seal_bytes(&Self::photo_aad(msg_id, b"meta"), &b)
    }

    fn unseal_meta(&self, msg_id: &Id, file_id: Id, sealed: &[u8]) -> Option<PhotoMeta> {
        let b = self.unseal_bytes(&Self::photo_aad(msg_id, b"meta"), sealed)?;
        if b.len() < 44 {
            return None;
        }
        let u = |i: usize| u32::from_be_bytes(b[i..i + 4].try_into().unwrap());
        Some(PhotoMeta { file_id, key: b[..32].try_into().unwrap(), size: u(32), w: u(36), h: u(40), thumb: b[44..].to_vec() })
    }

    /// Attaches a photo to a message. `data` = the JPEG when we already have it (we sent it).
    pub fn insert_photo(&self, msg_id: &Id, meta: &PhotoMeta, data: Option<&[u8]>) -> anyhow::Result<()> {
        let data = data.map(|d| self.seal_bytes(&Self::photo_aad(msg_id, b"data"), d));
        self.db.execute(
            "INSERT OR IGNORE INTO photos (msg_id, file_id, meta, data) VALUES (?1, ?2, ?3, ?4)",
            params![&msg_id.0[..], &meta.file_id.0[..], self.seal_meta(msg_id, meta), data],
        )?;
        Ok(())
    }

    /// A photo still to fetch, by its file id: (msg_id, meta, sent_at, peer).
    pub fn photo_by_file(&self, file_id: &Id) -> anyhow::Result<Option<(Id, PhotoMeta, i64, Id)>> {
        let row: Option<(Vec<u8>, Vec<u8>, i64, Vec<u8>)> = self
            .db
            .query_row(
                "SELECT p.msg_id, p.meta, m.sent_at, m.peer_user FROM photos p JOIN messages m ON m.msg_id = p.msg_id
                 WHERE p.file_id = ?1 AND p.data IS NULL AND p.gone = 0",
                [&file_id.0[..]],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        Ok(row.and_then(|(msg, meta, sent_at, peer)| {
            let msg = id(msg);
            self.unseal_meta(&msg, *file_id, &meta).map(|m| (msg, m, sent_at, id(peer)))
        }))
    }

    pub fn set_photo_data(&self, msg_id: &Id, data: &[u8]) -> anyhow::Result<()> {
        self.db.execute(
            "UPDATE photos SET data = ?1 WHERE msg_id = ?2",
            params![self.seal_bytes(&Self::photo_aad(msg_id, b"data"), data), &msg_id.0[..]],
        )?;
        Ok(())
    }

    pub fn mark_photo_gone(&self, msg_id: &Id) -> anyhow::Result<()> {
        self.db.execute("UPDATE photos SET gone = 1 WHERE msg_id = ?1", [&msg_id.0[..]])?;
        Ok(())
    }

    /// File ids of photos we still have to download.
    pub fn pending_downloads(&self) -> anyhow::Result<Vec<Id>> {
        let mut st = self.db.prepare("SELECT file_id FROM photos WHERE data IS NULL AND gone = 0")?;
        let rows = st.query_map([], |r| Ok(id(r.get(0)?)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// The full JPEG, if we have it.
    pub fn photo_data(&self, msg_id: &Id) -> anyhow::Result<Option<Vec<u8>>> {
        let data: Option<Option<Vec<u8>>> = self
            .db
            .query_row("SELECT data FROM photos WHERE msg_id = ?1", [&msg_id.0[..]], |r| r.get(0))
            .optional()?;
        Ok(data.flatten().and_then(|d| self.unseal_bytes(&Self::photo_aad(msg_id, b"data"), &d)))
    }

    pub fn add_upload(&self, file_id: &Id, msg_id: &Id) -> anyhow::Result<()> {
        self.db.execute(
            "INSERT OR IGNORE INTO uploads (file_id, msg_id) VALUES (?1, ?2)",
            params![&file_id.0[..], &msg_id.0[..]],
        )?;
        Ok(())
    }

    /// Photos to (re-)upload: those whose message still has undelivered copies in the outbox.
    /// Uploads whose copies are all done are forgotten.
    pub fn uploads_due(&self) -> anyhow::Result<Vec<Upload>> {
        self.db.execute("DELETE FROM uploads WHERE msg_id NOT IN (SELECT msg_id FROM outbox WHERE receipt = 0)", [])?;
        let mut st = self
            .db
            .prepare("SELECT u.file_id, u.msg_id, p.meta, p.data FROM uploads u JOIN photos p ON p.msg_id = u.msg_id")?;
        let rows = st.query_map([], |r| {
            Ok((id(r.get(0)?), id(r.get(1)?), r.get::<_, Vec<u8>>(2)?, r.get::<_, Option<Vec<u8>>>(3)?))
        })?;
        let mut out = vec![];
        for row in rows {
            let (file_id, msg_id, meta, data) = row?;
            let meta = self.unseal_meta(&msg_id, file_id, &meta);
            let data = data.and_then(|d| self.unseal_bytes(&Self::photo_aad(&msg_id, b"data"), &d));
            if let (Some(meta), Some(data)) = (meta, data) {
                out.push(Upload { file_id, key: meta.key, data });
            }
        }
        Ok(out)
    }

    /// Returns false if the message is already known (duplicate delivery).
    pub fn insert_message(&self, msg_id: &Id, peer_user: &Id, from_me: bool, text: &str, sent_at: i64, status: i64) -> anyhow::Result<bool> {
        let read = from_me as i64;
        let n = self.db.execute(
            "INSERT OR IGNORE INTO messages (msg_id, peer_user, from_me, sent_at, body, status, read) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![&msg_id.0[..], &peer_user.0[..], from_me, sent_at, self.seal(msg_id, text), status, read],
        )?;
        Ok(n > 0)
    }

    pub fn add_outbox(&self, id: &Id, msg_id: &Id, to_device: &Id, blob: &[u8], now: i64, receipt: bool) -> anyhow::Result<()> {
        self.db.execute(
            "INSERT OR REPLACE INTO outbox (id, msg_id, to_device, blob, created_at, receipt) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![&id.0[..], &msg_id.0[..], &to_device.0[..], blob, now, receipt],
        )?;
        Ok(())
    }

    pub fn outbox(&self) -> anyhow::Result<Vec<OutboxRow>> {
        let mut st = self.db.prepare("SELECT id, to_device, blob FROM outbox ORDER BY created_at")?;
        let rows = st.query_map([], |r| {
            Ok(OutboxRow { id: id(r.get(0)?), to_device: id(r.get(1)?), blob: r.get(2)? })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// The relay accepted a copy. Receipts need nothing more; messages become SENT.
    /// Returns the conversation to refresh, if a message changed state.
    pub fn on_stored(&self, transport_id: &Id) -> anyhow::Result<Option<Id>> {
        let row: Option<(Vec<u8>, bool)> = self
            .db
            .query_row("SELECT msg_id, receipt FROM outbox WHERE id = ?1", [&transport_id.0[..]], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?;
        let Some((msg_id, receipt)) = row else { return Ok(None) };
        if receipt {
            self.db.execute("DELETE FROM outbox WHERE id = ?1", [&transport_id.0[..]])?;
            return Ok(None);
        }
        let n = self.db.execute("UPDATE messages SET status = ?1 WHERE msg_id = ?2 AND status = ?3", params![SENT, &msg_id[..], SENDING])?;
        if n == 0 {
            return Ok(None);
        }
        self.peer_of(&id(msg_id))
    }

    /// E2E receipt from `device` for `msg_id`: its copy is done. `recipient` = the device
    /// belongs to the conversation partner (not one of our own devices).
    pub fn on_receipt(&self, msg_id: &Id, device: &Id, recipient: bool) -> anyhow::Result<Option<Id>> {
        self.db.execute("DELETE FROM outbox WHERE msg_id = ?1 AND to_device = ?2", params![&msg_id.0[..], &device.0[..]])?;
        if !recipient {
            return Ok(None);
        }
        let n = self.db.execute(
            "UPDATE messages SET status = ?1 WHERE msg_id = ?2 AND from_me = 1 AND status != ?1",
            params![DELIVERED, &msg_id.0[..]],
        )?;
        if n == 0 { Ok(None) } else { self.peer_of(msg_id) }
    }

    /// Drops copies older than the TTL; messages that never got a receipt become FAILED.
    /// Returns the affected conversations.
    pub fn expire(&self, now: i64) -> anyhow::Result<Vec<Id>> {
        let cutoff = now - OUTBOX_TTL_SECS;
        let mut st = self.db.prepare(
            "SELECT DISTINCT m.peer_user FROM outbox o JOIN messages m ON m.msg_id = o.msg_id
             WHERE o.created_at < ?1 AND m.status IN (?2, ?3)",
        )?;
        let peers: Vec<Id> = st.query_map(params![cutoff, SENDING, SENT], |r| Ok(id(r.get(0)?)))?.collect::<Result<_, _>>()?;
        self.db.execute(
            "UPDATE messages SET status = ?1 WHERE status IN (?2, ?3)
             AND msg_id IN (SELECT msg_id FROM outbox WHERE created_at < ?4)",
            params![FAILED, SENDING, SENT, cutoff],
        )?;
        self.db.execute("DELETE FROM outbox WHERE created_at < ?1", [cutoff])?;
        Ok(peers)
    }

    fn peer_of(&self, msg_id: &Id) -> anyhow::Result<Option<Id>> {
        Ok(self
            .db
            .query_row("SELECT peer_user FROM messages WHERE msg_id = ?1", [&msg_id.0[..]], |r| Ok(id(r.get(0)?)))
            .optional()?)
    }

    pub fn set_extra(&self, msg_id: &Id, extra: &Extra) -> anyhow::Result<()> {
        let json = serde_json::to_vec(extra)?;
        self.db.execute(
            "UPDATE messages SET extra = ?1 WHERE msg_id = ?2",
            params![self.seal_bytes(&Self::photo_aad(msg_id, b"extra"), &json), &msg_id.0[..]],
        )?;
        Ok(())
    }

    fn unseal_extra(&self, msg_id: &Id, sealed: Option<Vec<u8>>) -> Extra {
        sealed
            .and_then(|b| self.unseal_bytes(&Self::photo_aad(msg_id, b"extra"), &b))
            .and_then(|j| serde_json::from_slice(&j).ok())
            .unwrap_or_default()
    }

    /// Author of a message as a user id, with its conversation: (author, peer, deleted).
    fn author(&self, msg_id: &Id, my_user: &Id) -> anyhow::Result<Option<(Id, Id, bool)>> {
        let row: Option<(Vec<u8>, bool, bool)> = self
            .db
            .query_row(
                "SELECT peer_user, from_me, deleted FROM messages WHERE msg_id = ?1",
                [&msg_id.0[..]],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        Ok(row.map(|(peer, from_me, deleted)| {
            let peer = id(peer);
            (if from_me { *my_user } else { peer }, peer, deleted)
        }))
    }

    /// Applies an edit from `editor` if it wrote the message and the edit is newer than the
    /// last one. Returns the conversation to refresh.
    pub fn apply_edit(&self, msg_id: &Id, editor: &Id, my_user: &Id, text: &str, at: i64) -> anyhow::Result<Option<Id>> {
        let Some((author, peer, deleted)) = self.author(msg_id, my_user)? else { return Ok(None) };
        if author != *editor || deleted {
            return Ok(None);
        }
        let n = self.db.execute(
            "UPDATE messages SET body = ?1, edited_at = ?2 WHERE msg_id = ?3 AND (edited_at IS NULL OR edited_at <= ?2)",
            params![self.seal(msg_id, text), at, &msg_id.0[..]],
        )?;
        Ok((n > 0).then_some(peer))
    }

    /// Deletes a message for everyone, if `editor` wrote it: the text and photo are wiped,
    /// a "deleted" placeholder stays.
    /// Deletes a message for everyone: either side of the conversation may do it, and the
    /// message is gone without a trace (no placeholder).
    pub fn apply_delete(&self, msg_id: &Id, editor: &Id, my_user: &Id) -> anyhow::Result<Option<Id>> {
        let Some((_, peer, _)) = self.author(msg_id, my_user)? else { return Ok(None) };
        if *editor != peer && *editor != *my_user {
            return Ok(None);
        }
        self.delete_local(msg_id)
    }

    /// Sets `user`'s reaction (empty = none); newer wins.
    pub fn apply_react(&self, msg_id: &Id, user: &Id, my_user: &Id, emoji: &str, at: i64) -> anyhow::Result<Option<Id>> {
        let Some((_, peer, deleted)) = self.author(msg_id, my_user)? else { return Ok(None) };
        if deleted {
            return Ok(None);
        }
        let prev: Option<i64> = self
            .db
            .query_row(
                "SELECT at FROM reactions WHERE msg_id = ?1 AND user_id = ?2",
                params![&msg_id.0[..], &user.0[..]],
                |r| r.get(0),
            )
            .optional()?;
        if prev.is_some_and(|p| p > at) {
            return Ok(None);
        }
        if emoji.is_empty() {
            self.db.execute(
                "DELETE FROM reactions WHERE msg_id = ?1 AND user_id = ?2",
                params![&msg_id.0[..], &user.0[..]],
            )?;
        } else {
            let sealed = self.seal_bytes(&[&msg_id.0[..], &user.0[..], b"react"].concat(), emoji.as_bytes());
            self.db.execute(
                "INSERT OR REPLACE INTO reactions (msg_id, user_id, emoji, at) VALUES (?1, ?2, ?3, ?4)",
                params![&msg_id.0[..], &user.0[..], sealed, at],
            )?;
        }
        Ok(Some(peer))
    }

    /// Removes a message from this device only.
    pub fn delete_local(&self, msg_id: &Id) -> anyhow::Result<Option<Id>> {
        let peer = self.peer_of(msg_id)?;
        let tx = self.db.unchecked_transaction()?;
        for table in ["photos", "uploads", "reactions", "outbox", "messages"] {
            tx.execute(&format!("DELETE FROM {table} WHERE msg_id = ?1"), [&msg_id.0[..]])?;
        }
        tx.commit()?;
        Ok(peer)
    }

    /// The message as stored, for quoting and forwarding.
    pub fn original(&self, msg_id: &Id) -> anyhow::Result<Option<Original>> {
        let row: Option<(Vec<u8>, bool, Vec<u8>, bool)> = self
            .db
            .query_row(
                "SELECT peer_user, from_me, body, deleted FROM messages WHERE msg_id = ?1",
                [&msg_id.0[..]],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .optional()?;
        let Some((peer, from_me, body, deleted)) = row else { return Ok(None) };
        let photo: Option<(Vec<u8>, Vec<u8>, Option<Vec<u8>>)> = self
            .db
            .query_row("SELECT file_id, meta, data FROM photos WHERE msg_id = ?1", [&msg_id.0[..]], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })
            .optional()?;
        let photo = photo.and_then(|(file, meta, data)| {
            let meta = self.unseal_meta(msg_id, id(file), &meta)?;
            let data = data.and_then(|d| self.unseal_bytes(&Self::photo_aad(msg_id, b"data"), &d));
            Some((meta, data))
        });
        Ok(Some(Original { peer: id(peer), from_me, text: self.unseal(msg_id, &body), deleted, photo }))
    }

    fn reactions_of(&self, peer_user: &Id) -> anyhow::Result<std::collections::HashMap<Id, Vec<(Id, String)>>> {
        let mut st = self.db.prepare(
            "SELECT r.msg_id, r.user_id, r.emoji FROM reactions r JOIN messages m ON m.msg_id = r.msg_id
             WHERE m.peer_user = ?1 ORDER BY r.at",
        )?;
        let rows = st.query_map([&peer_user.0[..]], |r| {
            Ok((id(r.get(0)?), id(r.get(1)?), r.get::<_, Vec<u8>>(2)?))
        })?;
        let mut out: std::collections::HashMap<Id, Vec<(Id, String)>> = Default::default();
        for row in rows {
            let (msg, user, sealed) = row?;
            let aad = [&msg.0[..], &user.0[..], b"react"].concat();
            if let Some(e) = self.unseal_bytes(&aad, &sealed).and_then(|b| String::from_utf8(b).ok()) {
                out.entry(msg).or_default().push((user, e));
            }
        }
        Ok(out)
    }

    /// Newest last. `before` = sent_at bound for paging.
    pub fn messages(&self, peer_user: &Id, before: Option<i64>, limit: i64) -> anyhow::Result<Vec<Message>> {
        let mut st = self.db.prepare(
            "SELECT m.msg_id, m.from_me, m.sent_at, m.body, m.status, p.file_id, p.meta, p.data IS NOT NULL, p.gone,
                    m.edited_at IS NOT NULL, m.deleted, m.extra
             FROM messages m LEFT JOIN photos p ON p.msg_id = m.msg_id
             WHERE m.peer_user = ?1 AND m.sent_at < ?2 ORDER BY m.sent_at DESC, m.rowid DESC LIMIT ?3",
        )?;
        let mut reactions = self.reactions_of(peer_user)?;
        let rows = st.query_map(params![&peer_user.0[..], before.unwrap_or(i64::MAX), limit], |r| {
            let msg_id = id(r.get(0)?);
            let photo = match r.get::<_, Option<Vec<u8>>>(5)? {
                None => None,
                Some(file_id) => {
                    let meta: Vec<u8> = r.get(6)?;
                    let (ready, gone): (bool, bool) = (r.get(7)?, r.get(8)?);
                    Some(match self.unseal_meta(&msg_id, id(file_id), &meta) {
                        Some(m) => PhotoView { w: m.w, h: m.h, thumb: m.thumb, ready, gone },
                        None => PhotoView { w: 1, h: 1, thumb: vec![], ready: false, gone: true },
                    })
                }
            };
            Ok(Message {
                text: self.unseal(&msg_id, &r.get::<_, Vec<u8>>(3)?),
                msg_id,
                from_me: r.get(1)?,
                sent_at: r.get(2)?,
                status: r.get(4)?,
                photo,
                edited: r.get(9)?,
                deleted: r.get(10)?,
                extra: self.unseal_extra(&msg_id, r.get(11)?),
                reactions: reactions.remove(&msg_id).unwrap_or_default(),
            })
        })?;
        let mut out: Vec<Message> = rows.collect::<Result<_, _>>()?;
        out.reverse();
        Ok(out)
    }

    pub fn conversations(&self) -> anyhow::Result<Vec<Conversation>> {
        let mut st = self.db.prepare(
            "SELECT m.peer_user, m.msg_id, m.body, m.sent_at, m.from_me,
                    (SELECT COUNT(*) FROM messages u WHERE u.peer_user = m.peer_user AND u.read = 0),
                    EXISTS (SELECT 1 FROM photos p WHERE p.msg_id = m.msg_id), m.deleted
             FROM messages m
             WHERE m.rowid = (SELECT x.rowid FROM messages x WHERE x.peer_user = m.peer_user
                              ORDER BY x.sent_at DESC, x.rowid DESC LIMIT 1)
             ORDER BY m.sent_at DESC",
        )?;
        let rows = st.query_map([], |r| {
            let msg_id = id(r.get(1)?);
            Ok(Conversation {
                peer_user: id(r.get(0)?),
                last_text: self.unseal(&msg_id, &r.get::<_, Vec<u8>>(2)?),
                last_at: r.get(3)?,
                last_from_me: r.get(4)?,
                unread: r.get(5)?,
                last_photo: r.get(6)?,
                last_deleted: r.get(7)?,
            })
        })?;
        rows.map(|r| Ok(r?))
        .collect()
    }

    pub fn mark_read(&self, peer_user: &Id) -> anyhow::Result<()> {
        self.db.execute("UPDATE messages SET read = 1 WHERE peer_user = ?1 AND read = 0", [&peer_user.0[..]])?;
        Ok(())
    }

    /// Deletes a conversation and any undelivered copies of its messages (secure_delete is on).
    pub fn delete_conversation(&self, peer_user: &Id) -> anyhow::Result<()> {
        let tx = self.db.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM outbox WHERE msg_id IN (SELECT msg_id FROM messages WHERE peer_user = ?1)",
            [&peer_user.0[..]],
        )?;
        for table in ["photos", "uploads"] {
            tx.execute(
                &format!("DELETE FROM {table} WHERE msg_id IN (SELECT msg_id FROM messages WHERE peer_user = ?1)"),
                [&peer_user.0[..]],
            )?;
        }
        tx.execute("DELETE FROM messages WHERE peer_user = ?1", [&peer_user.0[..]])?;
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let s = Store::open(&dir.path().join("t.db"), [7; 32]).unwrap();
        (dir, s)
    }

    #[test]
    fn message_lifecycle() {
        let (_d, s) = store();
        let (peer, msg, dev_a, dev_own) = (Id([1; 16]), Id([2; 16]), Id([3; 16]), Id([4; 16]));
        assert!(s.insert_message(&msg, &peer, true, "привет", 100, SENDING).unwrap());
        assert!(!s.insert_message(&msg, &peer, true, "dup", 100, SENDING).unwrap());
        s.add_outbox(&Id([10; 16]), &msg, &dev_a, b"blob-a", 100, false).unwrap();
        s.add_outbox(&Id([11; 16]), &msg, &dev_own, b"blob-own", 100, false).unwrap();
        assert_eq!(s.outbox().unwrap().len(), 2);

        assert_eq!(s.on_stored(&Id([10; 16])).unwrap(), Some(peer));
        assert_eq!(s.messages(&peer, None, 10).unwrap()[0].status, SENT);
        // Own device's receipt removes its copy but doesn't mean "delivered".
        assert_eq!(s.on_receipt(&msg, &dev_own, false).unwrap(), None);
        assert_eq!(s.on_receipt(&msg, &dev_a, true).unwrap(), Some(peer));
        let m = &s.messages(&peer, None, 10).unwrap()[0];
        assert_eq!((m.status, m.text.as_str()), (DELIVERED, "привет"));
        assert!(s.outbox().unwrap().is_empty());
    }

    #[test]
    fn expiry_marks_failed_and_receipts_leave_on_stored() {
        let (_d, s) = store();
        let (peer, msg) = (Id([1; 16]), Id([2; 16]));
        s.insert_message(&msg, &peer, true, "x", 0, SENDING).unwrap();
        s.add_outbox(&Id([10; 16]), &msg, &Id([3; 16]), b"b", 0, false).unwrap();
        s.add_outbox(&Id([12; 16]), &Id([9; 16]), &Id([3; 16]), b"r", 1000, true).unwrap();
        assert_eq!(s.on_stored(&Id([12; 16])).unwrap(), None);
        assert_eq!(s.outbox().unwrap().len(), 1);
        assert_eq!(s.expire(OUTBOX_TTL_SECS + 1).unwrap(), vec![peer]);
        assert_eq!(s.messages(&peer, None, 10).unwrap()[0].status, FAILED);
        assert!(s.outbox().unwrap().is_empty());
    }

    #[test]
    fn photos_download_and_upload_lifecycle() {
        let (_d, s) = store();
        let (peer, msg, dev) = (Id([1; 16]), Id([2; 16]), Id([3; 16]));
        let meta = PhotoMeta { file_id: Id([4; 16]), key: [5; 32], size: 99, w: 30, h: 20, thumb: vec![1, 2, 3] };
        // Incoming: no data yet.
        s.insert_message(&msg, &peer, false, "", 10, RECEIVED).unwrap();
        s.insert_photo(&msg, &meta, None).unwrap();
        assert_eq!(s.pending_downloads().unwrap(), vec![meta.file_id]);
        let (m, got, sent_at, p) = s.photo_by_file(&meta.file_id).unwrap().unwrap();
        assert_eq!((m, &got, sent_at, p), (msg, &meta, 10, peer));
        let v = s.messages(&peer, None, 10).unwrap().remove(0).photo.unwrap();
        assert_eq!((v.w, v.h, v.ready, v.thumb.as_slice()), (30, 20, false, &[1u8, 2, 3][..]));
        s.set_photo_data(&msg, b"jpeg").unwrap();
        assert!(s.pending_downloads().unwrap().is_empty());
        assert!(s.photo_by_file(&meta.file_id).unwrap().is_none());
        assert_eq!(s.photo_data(&msg).unwrap().unwrap(), b"jpeg");
        assert!(s.conversations().unwrap()[0].last_photo);

        // Outgoing: uploaded while some copy is still in the outbox.
        let (msg2, file2) = (Id([6; 16]), Id([7; 16]));
        s.insert_message(&msg2, &peer, true, "cap", 11, SENDING).unwrap();
        s.insert_photo(&msg2, &PhotoMeta { file_id: file2, ..meta.clone() }, Some(b"mine")).unwrap();
        s.add_upload(&file2, &msg2).unwrap();
        assert!(s.uploads_due().unwrap().is_empty(), "no copies queued yet");
        s.add_upload(&file2, &msg2).unwrap();
        s.add_outbox(&Id([8; 16]), &msg2, &dev, b"b", 11, false).unwrap();
        let due = s.uploads_due().unwrap();
        assert_eq!((due[0].file_id, due[0].data.as_slice()), (file2, &b"mine"[..]));
        s.on_receipt(&msg2, &dev, true).unwrap();
        assert!(s.uploads_due().unwrap().is_empty());

        s.delete_conversation(&peer).unwrap();
        assert!(s.photo_data(&msg).unwrap().is_none());
    }

    #[test]
    fn conversations_and_unread() {
        let (_d, s) = store();
        let (a, b) = (Id([1; 16]), Id([5; 16]));
        s.insert_message(&Id([2; 16]), &a, false, "one", 10, RECEIVED).unwrap();
        s.insert_message(&Id([3; 16]), &a, false, "two", 20, RECEIVED).unwrap();
        s.insert_message(&Id([4; 16]), &b, true, "mine", 15, SENT).unwrap();
        let c = s.conversations().unwrap();
        assert_eq!((c[0].peer_user, c[0].last_text.as_str(), c[0].unread), (a, "two", 2));
        assert_eq!((c[1].peer_user, c[1].unread), (b, 0));
        s.mark_read(&a).unwrap();
        assert_eq!(s.conversations().unwrap()[0].unread, 0);
        s.delete_conversation(&b).unwrap();
        assert_eq!(s.conversations().unwrap().len(), 1);
        // Wrong key can't read bodies.
        let s2 = Store { db: Connection::open(s.db.path().unwrap()).unwrap(), key: [8; 32] };
        assert!(s2.messages(&a, None, 1).unwrap()[0].text.starts_with('⚠'));
    }
}
