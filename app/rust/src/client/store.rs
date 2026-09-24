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
}

pub struct Conversation {
    pub peer_user: Id,
    pub last_text: String,
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
";

fn id(b: Vec<u8>) -> Id {
    Id(b.try_into().unwrap_or([0; 16]))
}

impl Store {
    pub fn open(path: &Path, key: [u8; 32]) -> anyhow::Result<Store> {
        let db = Connection::open(path)?;
        db.pragma_update(None, "journal_mode", "WAL")?;
        db.pragma_update(None, "secure_delete", "ON")?;
        db.execute_batch(SCHEMA)?;
        Ok(Store { db, key })
    }

    fn seal(&self, msg_id: &Id, text: &str) -> Vec<u8> {
        let nonce: [u8; 24] = proto::random_bytes();
        let ct = XChaCha20Poly1305::new(&Array::from(self.key))
            .encrypt(&XNonce::from(nonce), Payload { msg: text.as_bytes(), aad: &msg_id.0 })
            .expect("encryption cannot fail");
        [&nonce[..], &ct].concat()
    }

    fn unseal(&self, msg_id: &Id, body: &[u8]) -> String {
        if body.len() < 24 {
            return String::new();
        }
        let (nonce, ct) = body.split_at(24);
        XChaCha20Poly1305::new(&Array::from(self.key))
            .decrypt(&XNonce::from(<[u8; 24]>::try_from(nonce).unwrap()), Payload { msg: ct, aad: &msg_id.0 })
            .ok()
            .and_then(|p| String::from_utf8(p).ok())
            .unwrap_or_else(|| "⚠ не удалось расшифровать".into())
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

    /// Newest last. `before` = sent_at bound for paging.
    pub fn messages(&self, peer_user: &Id, before: Option<i64>, limit: i64) -> anyhow::Result<Vec<Message>> {
        let mut st = self.db.prepare(
            "SELECT msg_id, from_me, sent_at, body, status FROM messages
             WHERE peer_user = ?1 AND sent_at < ?2 ORDER BY sent_at DESC, rowid DESC LIMIT ?3",
        )?;
        let rows = st.query_map(params![&peer_user.0[..], before.unwrap_or(i64::MAX), limit], |r| {
            Ok((id(r.get(0)?), r.get::<_, bool>(1)?, r.get::<_, i64>(2)?, r.get::<_, Vec<u8>>(3)?, r.get::<_, i64>(4)?))
        })?;
        let mut out: Vec<Message> = rows
            .map(|r| {
                r.map(|(msg_id, from_me, sent_at, body, status)| Message {
                    text: self.unseal(&msg_id, &body),
                    msg_id,
                    from_me,
                    sent_at,
                    status,
                })
            })
            .collect::<Result<_, _>>()?;
        out.reverse();
        Ok(out)
    }

    pub fn conversations(&self) -> anyhow::Result<Vec<Conversation>> {
        let mut st = self.db.prepare(
            "SELECT m.peer_user, m.msg_id, m.body, m.sent_at, m.from_me,
                    (SELECT COUNT(*) FROM messages u WHERE u.peer_user = m.peer_user AND u.read = 0)
             FROM messages m
             WHERE m.rowid = (SELECT x.rowid FROM messages x WHERE x.peer_user = m.peer_user
                              ORDER BY x.sent_at DESC, x.rowid DESC LIMIT 1)
             ORDER BY m.sent_at DESC",
        )?;
        let rows = st.query_map([], |r| {
            Ok((id(r.get(0)?), id(r.get(1)?), r.get::<_, Vec<u8>>(2)?, r.get::<_, i64>(3)?, r.get::<_, bool>(4)?, r.get::<_, i64>(5)?))
        })?;
        rows.map(|r| {
            let (peer_user, msg_id, body, last_at, last_from_me, unread) = r?;
            Ok(Conversation { peer_user, last_text: self.unseal(&msg_id, &body), last_at, last_from_me, unread })
        })
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
