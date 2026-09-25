use admin::rootkey::Kdf;
use admin::{Admin, DEFAULT_CERT_DAYS};
use proto::cert::Cert;
use proto::enroll::{self, Request};
use proto::manifest::Manifest;
use proto::{Id, SigningKey};

const FAST: Kdf = Kdf {
    m_cost_kib: 64,
    t_cost: 1,
    p_cost: 1,
};
const NOW: u64 = 1_800_000_000;

fn device(name: &str) -> (SigningKey, Request) {
    let k = proto::generate_signing_key();
    let r = Request {
        device_id: Id::random(),
        sign_pub: k.verifying_key().to_bytes(),
        enc_pub: [5; 32],
        name: name.into(),
    };
    // Round-trip through the text form, as it arrives from a chat.
    let text = enroll::to_text(enroll::REQUEST_PREFIX, &r.sign(&k).unwrap());
    let r = Request::verify(&enroll::from_text(enroll::REQUEST_PREFIX, &text).unwrap()).unwrap();
    (k, r)
}

fn setup() -> (tempfile::TempDir, Admin, SigningKey) {
    let dir = tempfile::tempdir().unwrap();
    let root = Admin::init(dir.path(), b"password1234", FAST).unwrap();
    let a = Admin::open(dir.path()).unwrap();
    (dir, a, root)
}

fn cert_of(text: &str) -> Vec<u8> {
    enroll::from_text(enroll::CERT_PREFIX, text).unwrap()
}

#[test]
fn init_unlock_and_no_overwrite() {
    let (dir, a, root) = setup();
    assert_eq!(
        a.unlock(b"password1234").unwrap().to_bytes(),
        root.to_bytes()
    );
    assert!(a.unlock(b"nope").is_err());
    assert!(Admin::init(dir.path(), b"password1234", FAST).is_err());
}

#[test]
fn enroll_new_user_then_second_device() {
    let (dir, mut a, root) = setup();
    let rp = root.verifying_key().to_bytes();
    let (_, r1) = device("Алиса");
    let i1 = a
        .enroll(&root, &r1, None, None, DEFAULT_CERT_DAYS, NOW)
        .unwrap();
    let c1 = Cert::verify(&rp, &cert_of(&i1.text), NOW).unwrap();
    assert_eq!(c1.name, "Алиса");
    assert_eq!(c1.devices.len(), 1);
    assert_eq!(c1.devices[0].sign_pub, r1.sign_pub);

    // Same second: issued_at must still increase.
    let (_, r2) = device("");
    let i2 = a
        .enroll(&root, &r2, None, Some(i1.user_id), DEFAULT_CERT_DAYS, NOW)
        .unwrap();
    let c2 = Cert::verify(&rp, &i2.cert, NOW).unwrap();
    assert_eq!(c2.user_id, c1.user_id);
    assert_eq!(c2.devices.len(), 2);
    assert!(c2.issued_at > c1.issued_at);

    // Duplicate device, empty name for a new user, unknown user.
    assert!(a.enroll(&root, &r2, None, None, 365, NOW).is_err());
    assert!(
        a.enroll(&root, &device("").1, None, None, 365, NOW)
            .is_err()
    );
    assert!(
        a.enroll(&root, &device("x").1, None, Some(Id::random()), 365, NOW)
            .is_err()
    );

    // State survives reopening; the stored cert text is the latest one.
    let a2 = Admin::open(dir.path()).unwrap();
    assert_eq!(a2.state, a.state);
    assert_eq!(a2.cert_text(&i1.user_id).unwrap(), i2.text);
}

#[test]
fn remove_device_and_revoke_user_feed_the_manifest() {
    let (_dir, mut a, root) = setup();
    let rp = root.verifying_key().to_bytes();
    let (_, r1) = device("bob");
    let (_, r2) = device("");
    let u = a.enroll(&root, &r1, None, None, 365, NOW).unwrap().user_id;
    a.enroll(&root, &r2, None, Some(u), 365, NOW).unwrap();
    let (_, r3) = device("carol");
    let carol = a.enroll(&root, &r3, None, None, 365, NOW).unwrap().user_id;

    // No server yet: signing must fail and not bump the version.
    assert!(a.sign_manifest(&root, NOW).is_err());
    assert_eq!(a.state.manifest.version, 0);
    let spub = hex::encode(proto::generate_signing_key().verifying_key().to_bytes());
    assert!(
        a.add_server("https://cdn.example.org/ws", &spub, "")
            .is_err()
    );
    a.add_server("wss://cdn.example.org/ws", &spub, "203.0.113.7")
        .unwrap();
    assert!(a.add_server("wss://other/ws", &spub, "").is_err());

    let new = a
        .remove_device(&root, &r2.device_id, 365, NOW)
        .unwrap()
        .unwrap();
    let c = Cert::verify(&rp, &new.cert, NOW).unwrap();
    assert_eq!(c.devices.len(), 1);
    assert!(c.device(&r2.device_id).is_none());
    // A removed device can't come back.
    assert!(a.enroll(&root, &r2, None, Some(u), 365, NOW).is_err());

    a.revoke_user(&carol).unwrap();
    assert!(
        a.enroll(&root, &device("").1, None, Some(carol), 365, NOW)
            .is_err()
    );

    let (v, path) = a.sign_manifest(&root, NOW).unwrap();
    assert_eq!(v, 1);
    let m = Manifest::verify(&rp, &std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(m.version, 1);
    assert!(m.is_revoked(&r2.device_id) && m.is_revoked(&carol) && !m.is_revoked(&u));
    assert_eq!(m.servers[0].ip_fallback, "203.0.113.7");
    assert_eq!(a.sign_manifest(&root, NOW).unwrap().0, 2);

    a.remove_server("wss://cdn.example.org/ws").unwrap();
    assert!(a.remove_server("wss://cdn.example.org/ws").is_err());
}

#[test]
fn removing_last_device_issues_nothing() {
    let (_dir, mut a, root) = setup();
    let (_, r) = device("dave");
    a.enroll(&root, &r, None, None, 365, NOW).unwrap();
    assert!(
        a.remove_device(&root, &r.device_id, 365, NOW)
            .unwrap()
            .is_none()
    );
    assert_eq!(a.revoked(), vec![r.device_id]);
}

#[test]
fn root_pub_is_readable_without_password() {
    let (dir, _a, root) = setup();
    let a = Admin::open(dir.path()).unwrap();
    assert_eq!(
        a.state.root_pub,
        hex::encode(root.verifying_key().to_bytes())
    );
}

#[test]
fn release_is_signed_and_mirrors_files() {
    let (dir, a, root) = setup();
    let apk = dir.path().join("in.apk");
    std::fs::write(&apk, b"apk bytes").unwrap();
    std::fs::create_dir_all(dir.path().join("releases")).unwrap();
    std::fs::write(dir.path().join("releases").join("old.apk"), b"x").unwrap();
    let path = a
        .sign_release(&root, "0.1.4", 4, &[("android-arm64", apk.as_path(), "botato-0.1.4.apk".into())], 100)
        .unwrap();
    let rel = proto::release::Release::verify(root.verifying_key().as_bytes(), &std::fs::read(path).unwrap()).unwrap();
    assert_eq!((rel.version.as_str(), rel.build, rel.files[0].size), ("0.1.4", 4, 9));
    assert!(dir.path().join("releases").join("botato-0.1.4.apk").exists());
    assert!(!dir.path().join("releases").join("old.apk").exists());
}
