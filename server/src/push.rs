//! Empty wake-up pushes. Payloads never carry content.

use proto::frames::Platform;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PushKind {
    Message,
    Call,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PushToken {
    pub platform: Platform,
    pub token: String,
}

/// Must not block: implementations spawn their own I/O.
pub trait Pusher: Send + Sync {
    fn push(&self, token: &PushToken, kind: PushKind);
}

/// Used until FCM/APNs are wired up.
pub struct NoopPusher;

impl Pusher for NoopPusher {
    fn push(&self, _: &PushToken, _: PushKind) {}
}
