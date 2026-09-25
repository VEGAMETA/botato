//! Test vectors in `protocol/vectors/` are the contract for every implementation.
//! Regenerate (only when the format intentionally changes): `UPDATE_VECTORS=1 cargo test -p proto`.

use std::path::PathBuf;

use proto::auth::{self, Nonce};
use proto::cert::{Cert, Device};
use proto::manifest::{Manifest, ServerEntry};
use proto::{Error, Id, SigningKey};
use serde_json::{Value, json};

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../protocol/vectors")
}

fn key(seed_byte: u8) -> SigningKey {
    SigningKey::from_bytes(&[seed_byte; 32])
}

fn id(b: u8) -> Id {
    Id([b; 16])
}

fn check(name: &str, v: Value) {
    let path = dir().join(name);
    let rendered = serde_json::to_string_pretty(&v).unwrap() + "\n";
    if std::env::var_os("UPDATE_VECTORS").is_some() {
        std::fs::create_dir_all(dir()).unwrap();
        std::fs::write(&path, rendered).unwrap();
        return;
    }
    let stored = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("missing {name}; run with UPDATE_VECTORS=1"));
    let stored: Value = serde_json::from_str(&stored).unwrap();
    assert_eq!(stored, v, "{name} differs from the stored vector");
}

fn sample_cert() -> Cert {
    Cert {
        user_id: id(0x11),
        name: "Алиса".into(),
        issued_at: 1_790_000_000,
        expires_at: 1_821_536_000,
        devices: vec![
            Device {
                device_id: id(0x21),
                sign_pub: key(0x31).verifying_key().to_bytes(),
                enc_pub: [0x41; 32],
            },
            Device {
                device_id: id(0x22),
                sign_pub: key(0x32).verifying_key().to_bytes(),
                enc_pub: [0x42; 32],
            },
        ],
    }
}

#[test]
fn cert_vector() {
    let root = key(0x01);
    let c = sample_cert();
    let body = c.encode_body().unwrap();
    let signed = c.sign(&root).unwrap();
    let root_pub = root.verifying_key().to_bytes();
    let now = 1_800_000_000;
    assert_eq!(Cert::verify(&root_pub, &signed, now).unwrap(), c);

    let mut tampered = signed.clone();
    tampered[20] ^= 1;
    let mut trailing = body.clone();
    trailing.push(0);
    let trailing_signed = [
        trailing.clone(),
        proto_sign(&root, proto::ctx::CERT, &trailing),
    ]
    .concat();

    let invalid = [
        ("tampered body", tampered, now, Error::Signature),
        ("expired", signed.clone(), c.expires_at, Error::Expired),
        (
            "not yet valid",
            signed.clone(),
            c.issued_at - 301,
            Error::NotYetValid,
        ),
        ("trailing byte", trailing_signed, now, Error::Trailing),
        (
            "other root",
            c.sign(&key(0x02)).unwrap(),
            now,
            Error::Signature,
        ),
    ];
    let mut inv = vec![];
    for (desc, b, at, err) in invalid {
        assert_eq!(Cert::verify(&root_pub, &b, at), Err(err.clone()), "{desc}");
        inv.push(json!({"desc": desc, "signed": hex::encode(&b), "now": at, "error": format!("{err:?}")}));
    }

    check(
        "cert.json",
        json!({
            "root_seed": hex::encode([0x01u8; 32]),
            "root_pub": hex::encode(root_pub),
            "cert": {
                "user_id": c.user_id.to_hex(),
                "name": c.name,
                "issued_at": c.issued_at,
                "expires_at": c.expires_at,
                "devices": c.devices.iter().map(|d| json!({
                    "device_id": d.device_id.to_hex(),
                    "sign_pub": hex::encode(d.sign_pub),
                    "enc_pub": hex::encode(d.enc_pub),
                })).collect::<Vec<_>>(),
            },
            "ctx": hex::encode(proto::ctx::CERT),
            "body": hex::encode(&body),
            "signed": hex::encode(&signed),
            "now": now,
            "invalid": inv,
        }),
    );
}

fn proto_sign(k: &SigningKey, ctx: &[u8], body: &[u8]) -> Vec<u8> {
    use ed25519_dalek::Signer;
    k.sign(&[ctx, body].concat()).to_bytes().to_vec()
}

#[test]
fn manifest_vector() {
    let root = key(0x01);
    let m = Manifest {
        version: 7,
        issued_at: 1_790_000_000,
        servers: vec![ServerEntry {
            url: "wss://relay.example.org/ws".into(),
            ip_fallback: "".into(),
            server_pub: key(0x51).verifying_key().to_bytes(),
        }],
        stun: vec!["stun:stun.example.org:3478".into()],
        nostr_relays: vec!["wss://nostr.example.org".into()],
        revoked: vec![id(0x61), id(0x62)],
    };
    let body = m.encode_body().unwrap();
    let signed = m.sign(&root).unwrap();
    let root_pub = root.verifying_key().to_bytes();
    assert_eq!(Manifest::verify(&root_pub, &signed).unwrap(), m);
    let mut tampered = signed.clone();
    tampered[1] ^= 1;
    assert_eq!(
        Manifest::verify(&root_pub, &tampered),
        Err(Error::Signature)
    );

    check(
        "manifest.json",
        json!({
            "root_pub": hex::encode(root_pub),
            "manifest": {
                "version": m.version,
                "issued_at": m.issued_at,
                "servers": m.servers.iter().map(|s| json!({
                    "url": s.url, "ip_fallback": s.ip_fallback, "server_pub": hex::encode(s.server_pub),
                })).collect::<Vec<_>>(),
                "stun": m.stun,
                "nostr_relays": m.nostr_relays,
                "revoked": m.revoked.iter().map(Id::to_hex).collect::<Vec<_>>(),
            },
            "ctx": hex::encode(proto::ctx::MANIFEST),
            "body": hex::encode(&body),
            "signed": hex::encode(&signed),
        }),
    );
}

#[test]
fn auth_vector() {
    let server = key(0x51);
    let device = key(0x31);
    let server_pub = server.verifying_key().to_bytes();
    let cn: Nonce = [0xa1; 32];
    let sn: Nonce = [0xb2; 32];
    let ssig = auth::sign_server(&server, &cn, &sn);
    let csig = auth::sign_client(&device, &sn, &server_pub, &cn);
    auth::verify_server(&server_pub, &cn, &sn, &ssig).unwrap();
    auth::verify_client(
        &device.verifying_key().to_bytes(),
        &sn,
        &server_pub,
        &cn,
        &csig,
    )
    .unwrap();
    // Swapped nonces and a different server key must fail.
    assert!(auth::verify_server(&server_pub, &sn, &cn, &ssig).is_err());
    let other = key(0x52).verifying_key().to_bytes();
    assert!(
        auth::verify_client(&device.verifying_key().to_bytes(), &sn, &other, &cn, &csig).is_err()
    );

    check(
        "auth.json",
        json!({
            "server_seed": hex::encode([0x51u8; 32]),
            "server_pub": hex::encode(server_pub),
            "device_seed": hex::encode([0x31u8; 32]),
            "device_pub": hex::encode(device.verifying_key().to_bytes()),
            "client_nonce": hex::encode(cn),
            "server_nonce": hex::encode(sn),
            "server_ctx": hex::encode(proto::ctx::AUTH_SERVER),
            "server_sig": hex::encode(ssig),
            "client_ctx": hex::encode(proto::ctx::AUTH_CLIENT),
            "client_sig": hex::encode(csig),
        }),
    );
}

#[test]
fn enroll_vector() {
    use proto::enroll::{self, Request};
    let device = key(0x31);
    let req = Request {
        device_id: id(0x21),
        sign_pub: device.verifying_key().to_bytes(),
        enc_pub: [0x41; 32],
        name: "Алиса".into(),
    };
    let signed = req.sign(&device).unwrap();
    assert_eq!(Request::verify(&signed).unwrap(), req);
    let text = enroll::to_text(enroll::REQUEST_PREFIX, &signed);
    assert_eq!(
        enroll::from_text(
            enroll::REQUEST_PREFIX,
            &format!(" {}\n{} ", &text[..40], &text[40..])
        )
        .unwrap(),
        signed
    );
    assert!(enroll::from_text(enroll::CERT_PREFIX, &text).is_err());
    // Signed by a key other than sign_pub.
    assert!(req.sign(&key(0x32)).is_err());
    let mut forged = signed.clone();
    forged[20] ^= 1;
    assert!(Request::verify(&forged).is_err());

    check(
        "enroll.json",
        json!({
            "device_seed": hex::encode([0x31u8; 32]),
            "request": {
                "device_id": req.device_id.to_hex(),
                "sign_pub": hex::encode(req.sign_pub),
                "enc_pub": hex::encode(req.enc_pub),
                "name": req.name,
            },
            "ctx": hex::encode(proto::ctx::ENROLL),
            "signed": hex::encode(&signed),
            "text": text,
            "fingerprint_ctx": hex::encode(proto::ctx::FINGERPRINT),
            "fingerprint": req.fingerprint(),
        }),
    );
}

#[test]
fn envelope_vector() {
    use proto::device::DeviceSecret;
    use proto::envelope::{self, KIND_SIGNAL};
    let secret = |id: u8, s: u8, e: u8| {
        let mut b = vec![1u8];
        b.extend_from_slice(&[id; 16]);
        b.extend_from_slice(&[s; 32]);
        b.extend_from_slice(&[e; 32]);
        DeviceSecret::from_bytes(&b).unwrap()
    };
    let sender = secret(0x21, 0x31, 0x41);
    let recipient = secret(0x22, 0x32, 0x42);
    let eph = [0x71u8; 32];
    let nonce = [0x72u8; 24];
    let payload = br#"{"type":"offer","sdp":"v=0"}"#;
    let blob = envelope::seal_with(
        &sender,
        &recipient.device_id,
        &recipient.enc_pub(),
        KIND_SIGNAL,
        1_800_000_000,
        payload,
        x25519_dalek::StaticSecret::from(eph),
        nonce,
    )
    .unwrap();
    let opened = envelope::open(&recipient, &blob).unwrap().verify(&sender.sign_pub()).unwrap();
    assert_eq!(opened.payload, payload);

    check(
        "envelope.json",
        json!({
            "sender_secret": hex::encode(sender.to_bytes().as_slice()),
            "sender_sign_pub": hex::encode(sender.sign_pub()),
            "recipient_secret": hex::encode(recipient.to_bytes().as_slice()),
            "recipient_device_id": recipient.device_id.to_hex(),
            "recipient_enc_pub": hex::encode(recipient.enc_pub()),
            "ephemeral_secret": hex::encode(eph),
            "nonce": hex::encode(nonce),
            "kind": KIND_SIGNAL,
            "sent_at": 1_800_000_000u64,
            "payload": String::from_utf8_lossy(payload),
            "ctx": hex::encode(proto::ctx::ENVELOPE),
            "blob": hex::encode(&blob),
        }),
    );
}

#[test]
fn file_vector() {
    use proto::file;
    let key = [0x51u8; 32];
    let file_id = id(0x52);
    let nonce = [0x53u8; 24];
    let plaintext = b"not really a jpeg";
    let blob = file::seal_with(&key, &file_id, plaintext, nonce).unwrap();
    assert_eq!(file::open(&key, &file_id, &blob).unwrap(), plaintext);
    check(
        "file.json",
        json!({
            "key": hex::encode(key),
            "file_id": file_id.to_hex(),
            "nonce": hex::encode(nonce),
            "aad_ctx": hex::encode(proto::ctx::FILE),
            "plaintext": hex::encode(plaintext),
            "blob": hex::encode(&blob),
        }),
    );
}

#[test]
fn release_vector() {
    use proto::release::{ANDROID_ARM64, Release, ReleaseFile, WINDOWS_X64};
    let root = key(0x61);
    let rel = Release {
        version: "0.1.4".into(),
        build: 4,
        issued_at: 1_800_000_000,
        files: vec![
            ReleaseFile { platform: ANDROID_ARM64.into(), file: "botato-0.1.4.apk".into(), size: 38_000_000, sha256: [0x62; 32] },
            ReleaseFile { platform: WINDOWS_X64.into(), file: "botato-setup-0.1.4.exe".into(), size: 19_500_000, sha256: [0x63; 32] },
        ],
    };
    let signed = rel.sign(&root).unwrap();
    assert_eq!(Release::verify(root.verifying_key().as_bytes(), &signed).unwrap(), rel);
    check(
        "release.json",
        json!({
            "root_pub": hex::encode(root.verifying_key().to_bytes()),
            "ctx": hex::encode(proto::ctx::RELEASE),
            "version": rel.version,
            "build": rel.build,
            "issued_at": rel.issued_at,
            "signed": hex::encode(&signed),
        }),
    );
}
