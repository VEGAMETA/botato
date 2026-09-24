use proto::cert::{Cert, Device};
use proto::frames::{ClientFrame, ErrorCode, ServerFrame};
use proto::manifest::{Manifest, ServerEntry};
use proto::{Error, Id};

fn cert(devices: usize) -> Cert {
    Cert {
        user_id: Id([1; 16]),
        name: "bob".into(),
        issued_at: 10,
        expires_at: 20,
        devices: (0..devices)
            .map(|i| Device {
                device_id: Id([i as u8 + 2; 16]),
                sign_pub: [0; 32],
                enc_pub: [0; 32],
            })
            .collect(),
    }
}

#[test]
fn cert_rejects_invalid_fields() {
    assert_eq!(cert(0).encode_body(), Err(Error::Invalid));
    assert_eq!(cert(9).encode_body(), Err(Error::Invalid));
    assert!(cert(8).encode_body().is_ok());

    let mut dup = cert(2);
    dup.devices[1].device_id = dup.devices[0].device_id;
    assert_eq!(dup.encode_body(), Err(Error::Invalid));

    let mut long = cert(1);
    long.name = "x".repeat(65);
    assert_eq!(long.encode_body(), Err(Error::Invalid));

    let mut backwards = cert(1);
    backwards.expires_at = backwards.issued_at;
    assert_eq!(backwards.encode_body(), Err(Error::Invalid));
}

#[test]
fn verify_rejects_garbage() {
    let root = proto::generate_signing_key();
    let pub_ = root.verifying_key().to_bytes();
    assert_eq!(Cert::verify(&pub_, &[0; 10], 0), Err(Error::Truncated));
    assert_eq!(Manifest::verify(&pub_, &[0; 70]), Err(Error::Signature));
    // Wrong key length is an error, not a panic.
    assert!(Cert::verify(&[0; 5], &cert(1).sign(&root).unwrap(), 15).is_err());
}

#[test]
fn manifest_requires_a_server() {
    assert_eq!(Manifest::default().encode_body(), Err(Error::Invalid));
    let m = Manifest {
        servers: vec![ServerEntry {
            url: "wss://a/ws".into(),
            ip_fallback: "".into(),
            server_pub: [9; 32],
        }],
        ..Default::default()
    };
    assert!(m.has_server(&[9; 32]));
    assert!(!m.has_server(&[8; 32]));
}

#[test]
fn frames_json_shape() {
    let f = ClientFrame::Send {
        to: Id([0xab; 16]),
        id: Id([1; 16]),
        blob: vec![1, 2, 3],
    };
    let s = serde_json::to_string(&f).unwrap();
    assert_eq!(
        s,
        r#"{"t":"send","to":"abababababababababababababababab","id":"01010101010101010101010101010101","blob":"AQID"}"#
    );
    assert_eq!(serde_json::from_str::<ClientFrame>(&s).unwrap(), f);

    assert_eq!(
        serde_json::from_str::<ClientFrame>(r#"{"t":"turn"}"#).unwrap(),
        ClientFrame::Turn {}
    );
    let e = ServerFrame::Error {
        r#ref: "send".into(),
        id: None,
        code: ErrorCode::RateLimited,
    };
    assert_eq!(
        serde_json::to_string(&e).unwrap(),
        r#"{"t":"error","ref":"send","code":"rate_limited"}"#
    );

    let up = ClientFrame::Upload { id: Id([2; 16]), data: vec![9] };
    assert_eq!(
        serde_json::to_string(&up).unwrap(),
        r#"{"t":"upload","id":"02020202020202020202020202020202","data":"CQ=="}"#
    );
    let f = ServerFrame::File { id: Id([2; 16]), data: vec![9] };
    assert_eq!(
        serde_json::to_string(&f).unwrap(),
        r#"{"t":"file","id":"02020202020202020202020202020202","data":"CQ=="}"#
    );
    assert_eq!(
        serde_json::to_string(&ServerFrame::Error { r#ref: "download".into(), id: Some(Id([2; 16])), code: ErrorCode::NotFound }).unwrap(),
        r#"{"t":"error","ref":"download","id":"02020202020202020202020202020202","code":"not_found"}"#
    );

    assert_eq!(
        serde_json::to_string(&ServerFrame::Presence { online: vec![Id([3; 16])] }).unwrap(),
        r#"{"t":"presence","online":["03030303030303030303030303030303"]}"#
    );

    // Uppercase hex, unknown fields, unknown types and bad base64 are rejected.
    for bad in [
        r#"{"t":"ack","id":"ABABABABABABABABABABABABABABABAB"}"#,
        r#"{"t":"ack","id":"abababababababababababababababab","x":1}"#,
        r#"{"t":"nope"}"#,
        r#"{"t":"signal","to":"abababababababababababababababab","blob":"!!"}"#,
    ] {
        assert!(serde_json::from_str::<ClientFrame>(bad).is_err(), "{bad}");
    }
}
