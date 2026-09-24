//! In-memory mailboxes, one per device. Nothing here survives a restart — by design:
//! senders keep messages in their outbox until an E2E receipt arrives.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use proto::Id;

#[derive(Clone, Debug)]
pub struct Limits {
    pub ttl_secs: u64,
    pub per_sender_count: usize,
    pub per_sender_bytes: usize,
    pub per_rcpt_count: usize,
    pub per_rcpt_bytes: usize,
    /// All mailboxes together; the relay runs on small VPSes.
    pub total_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            ttl_secs: 24 * 3600,
            per_sender_count: 1000,
            per_sender_bytes: 16 << 20,
            per_rcpt_count: 2000,
            per_rcpt_bytes: 32 << 20,
            total_bytes: 128 << 20,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Msg {
    pub seq: u64,
    pub id: Id,
    pub from: Id,
    pub blob: Arc<[u8]>,
    pub expires: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub struct QuotaExceeded;

#[derive(Default)]
struct Inbox {
    msgs: BTreeMap<u64, Msg>,
    by_id: HashMap<Id, u64>,
    bytes: usize,
}

#[derive(Default, Clone, Copy)]
struct Usage {
    count: usize,
    bytes: usize,
}

pub struct Mailbox {
    limits: Limits,
    next_seq: u64,
    inboxes: HashMap<Id, Inbox>,
    senders: HashMap<Id, Usage>,
}

impl Mailbox {
    pub fn new(limits: Limits) -> Self {
        Mailbox {
            limits,
            next_seq: 1,
            inboxes: HashMap::new(),
            senders: HashMap::new(),
        }
    }

    /// Stores a message. Re-sending the same `(rcpt, id)` is an idempotent success.
    pub fn put(
        &mut self,
        rcpt: Id,
        from: Id,
        id: Id,
        blob: &[u8],
        now: u64,
    ) -> Result<(), QuotaExceeded> {
        let l = &self.limits;
        let inbox = self.inboxes.get(&rcpt);
        if inbox.is_some_and(|i| i.by_id.contains_key(&id)) {
            return Ok(());
        }
        let size = blob.len();
        let s = self.senders.get(&from).copied().unwrap_or_default();
        let (rc, rb) = inbox.map_or((0, 0), |i| (i.msgs.len(), i.bytes));
        if s.count + 1 > l.per_sender_count
            || s.bytes + size > l.per_sender_bytes
            || rc + 1 > l.per_rcpt_count
            || rb + size > l.per_rcpt_bytes
            || self.senders.values().map(|u| u.bytes).sum::<usize>() + size > l.total_bytes
        {
            return Err(QuotaExceeded);
        }
        let seq = self.next_seq;
        self.next_seq += 1;
        let msg = Msg {
            seq,
            id,
            from,
            blob: blob.into(),
            expires: now + l.ttl_secs,
        };
        let inbox = self.inboxes.entry(rcpt).or_default();
        inbox.by_id.insert(id, seq);
        inbox.bytes += size;
        inbox.msgs.insert(seq, msg);
        let u = self.senders.entry(from).or_default();
        u.count += 1;
        u.bytes += size;
        Ok(())
    }

    /// Up to `limit` unexpired messages for `rcpt` with `seq > after`, in order.
    pub fn after(&self, rcpt: &Id, after: u64, limit: usize, now: u64) -> Vec<Msg> {
        let Some(inbox) = self.inboxes.get(rcpt) else {
            return vec![];
        };
        inbox
            .msgs
            .range(after + 1..)
            .map(|(_, m)| m)
            .filter(|m| m.expires > now)
            .take(limit)
            .cloned()
            .collect()
    }

    /// Only the recipient can ack, so the key is `(rcpt, id)`.
    pub fn ack(&mut self, rcpt: &Id, id: &Id) {
        let Some(inbox) = self.inboxes.get_mut(rcpt) else {
            return;
        };
        if let Some(seq) = inbox.by_id.remove(id)
            && let Some(m) = inbox.msgs.remove(&seq)
        {
            inbox.bytes -= m.blob.len();
            Self::release(&mut self.senders, &m);
        }
        if inbox.msgs.is_empty() {
            self.inboxes.remove(rcpt);
        }
    }

    /// Drops expired messages; returns how many.
    pub fn expire(&mut self, now: u64) -> usize {
        self.retain(|m| m.expires > now)
    }

    /// Drops everything addressed to or sent by any of `ids` (revocation).
    pub fn purge(&mut self, ids: &[Id]) {
        for id in ids {
            if let Some(inbox) = self.inboxes.remove(id) {
                for m in inbox.msgs.values() {
                    Self::release(&mut self.senders, m);
                }
            }
        }
        self.retain(|m| !ids.contains(&m.from));
    }

    pub fn is_empty(&self) -> bool {
        self.inboxes.is_empty()
    }

    pub fn len(&self) -> usize {
        self.inboxes.values().map(|i| i.msgs.len()).sum()
    }

    fn retain(&mut self, keep: impl Fn(&Msg) -> bool) -> usize {
        let mut removed = 0;
        let senders = &mut self.senders;
        self.inboxes.retain(|_, inbox| {
            let gone: Vec<u64> = inbox
                .msgs
                .values()
                .filter(|m| !keep(m))
                .map(|m| m.seq)
                .collect();
            for seq in gone {
                let m = inbox.msgs.remove(&seq).unwrap();
                inbox.by_id.remove(&m.id);
                inbox.bytes -= m.blob.len();
                Self::release(senders, &m);
                removed += 1;
            }
            !inbox.msgs.is_empty()
        });
        removed
    }

    fn release(senders: &mut HashMap<Id, Usage>, m: &Msg) {
        if let Some(u) = senders.get_mut(&m.from) {
            u.count -= 1;
            u.bytes -= m.blob.len();
            if u.count == 0 {
                senders.remove(&m.from);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(b: u8) -> Id {
        Id([b; 16])
    }

    #[test]
    fn put_after_ack() {
        let mut mb = Mailbox::new(Limits::default());
        mb.put(id(1), id(9), id(100), b"a", 0).unwrap();
        mb.put(id(1), id(9), id(101), b"b", 0).unwrap();
        mb.put(id(1), id(9), id(100), b"dup", 0).unwrap(); // idempotent
        let got = mb.after(&id(1), 0, 10, 0);
        assert_eq!(got.len(), 2);
        assert_eq!(&*got[0].blob, b"a");
        assert_eq!(mb.after(&id(1), got[0].seq, 10, 0).len(), 1);
        mb.ack(&id(1), &id(100));
        mb.ack(&id(2), &id(101)); // not the recipient: no effect
        assert_eq!(mb.len(), 1);
    }

    #[test]
    fn quotas() {
        let l = Limits {
            per_sender_count: 2,
            per_rcpt_bytes: 10,
            ..Default::default()
        };
        let mut mb = Mailbox::new(l);
        mb.put(id(1), id(9), id(100), b"x", 0).unwrap();
        mb.put(id(2), id(9), id(101), b"x", 0).unwrap();
        assert_eq!(mb.put(id(3), id(9), id(102), b"x", 0), Err(QuotaExceeded));
        mb.ack(&id(1), &id(100));
        mb.put(id(3), id(9), id(102), b"x", 0).unwrap();
        assert_eq!(
            mb.put(id(4), id(8), id(103), &[0; 11], 0),
            Err(QuotaExceeded)
        );

        let mut mb = Mailbox::new(Limits {
            total_bytes: 3,
            ..Default::default()
        });
        mb.put(id(1), id(8), id(100), b"ab", 0).unwrap();
        assert_eq!(mb.put(id(2), id(9), id(101), b"ab", 0), Err(QuotaExceeded));
    }

    #[test]
    fn expiry_and_purge() {
        let l = Limits {
            ttl_secs: 100,
            ..Default::default()
        };
        let mut mb = Mailbox::new(l);
        mb.put(id(1), id(9), id(100), b"x", 0).unwrap();
        mb.put(id(1), id(8), id(101), b"x", 50).unwrap();
        mb.put(id(2), id(9), id(102), b"x", 50).unwrap();
        assert_eq!(mb.after(&id(1), 0, 10, 100).len(), 1); // expired ones are hidden
        assert_eq!(mb.expire(100), 1);
        mb.purge(&[id(9)]);
        assert_eq!(mb.len(), 1);
        mb.purge(&[id(1)]);
        assert_eq!(mb.len(), 0);
        assert!(mb.senders.is_empty());
    }
}
