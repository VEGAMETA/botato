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
