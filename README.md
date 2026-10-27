# botato

A private messenger for a small circle (3–5 people). The main thing is voice and video calls with end-to-end encryption and minimal latency; text messages come second. Accounts exist only by the admin's invitation, and the whole backend is one server that can be replaced in minutes.

## Principles

- **The server is disposable.** Keys, certificates, history and the outbox live on the clients. Losing the server loses at most undelivered messages from the last 24 hours.
- **The server sees no content.** It handles encrypted blobs, public keys and routing metadata only.
- **Media goes peer-to-peer.** TURN is a fallback, not the default path.
- **No home-made crypto.** Ed25519, X25519, HKDF-SHA256 and XChaCha20-Poly1305 from audited Rust crates; signatures cover canonical bytes, never JSON.
- **Nothing on disk, nothing older than 24 hours.** Mailboxes live in RAM. Photo attachments are the only exception: E2E-encrypted files, encrypted once more with a key that exists only in the relay's memory, wiped on start.
- **No third parties.** No analytics, crash reporters or call SDKs; push notifications are empty wake-ups.

## How it works

```
 Admin ──► offline Ed25519 root key: signs certificates, the server manifest and app releases
              │ public part is built into the client
              ▼
 Client A ◄════ P2P media (DTLS-SRTP) ════► Client B
    │  E2E blobs, signed signalling           │
    └──────────► relay (untrusted) ◄──────────┘
                 coturn (untrusted, fallback)
                 Nostr relays (untrusted, fallback signalling)
```

- The client is the only source of truth: it verifies certificates, the manifest, SDP signatures and the server's identity before revealing its own certificate.
- The relay is a dumb postman. A compromised relay can delay or drop messages and see who talks to whom, but cannot read messages or impersonate users.
- Devices are enrolled with a QR request and a 20-digit fingerprint checked by voice; the admin issues one certificate per user with up to 8 devices.
- The signed manifest (servers, STUN, Nostr relays, revocations) is distributed through the build, the server, Nostr and a static mirror, so clients can move to a new server on their own.
- App updates are signed with the root key; a client installs a file only if its size and SHA-256 match the signed release.

The wire format, signed structures and test vectors are in [`protocol/README.md`](protocol/README.md).

## Stack

| Part | Technologies |
|---|---|
| Client (`app/`) | Flutter for Android, Windows, Linux and macOS; `flutter_webrtc`; own Kotlin services for calls, notifications and self-update; WASAPI microphone capture on Windows |
| Protocol (`crates/proto`) | Rust, shared by the client (via `flutter_rust_bridge`), the relay and the admin tools |
| Relay (`server/`) | Rust, one static binary: `tokio`, `axum` WebSocket, in-memory storage, no database |
| Admin (`tools/admin`, `tools/botatoadmin`) | Rust CLI (`clap`) and GUI (`egui`): root key, certificates, manifest, revocation, release signing and rollout |
| Infrastructure (`infra/`) | Debian 13, Docker Compose, Caddy (automatic TLS), coturn, nftables, SSH behind fwknop, Ansible |

## Repository layout

```
protocol/          format specification and JSON test vectors (the source of truth)
crates/proto/      Rust implementation of protocol/, checked against the vectors
server/            relay: auth, mailboxes, signalling, push, TURN credentials, attachments
tools/admin/       admin CLI
tools/botatoadmin/ admin GUI, including "release and roll out"
app/               Flutter client; app/rust is the bridge crate (relay client, SQLite history and outbox)
infra/             relay Dockerfile and Ansible (bootstrap, deploy; roles base, docker, app, fwknop, firewall)
```

## Build and test

```sh
cargo test --workspace
cd app && flutter test

cargo run -p admin -- --help           # admin data lives offline; back up the root key
cargo run --release -p botatoadmin     # admin GUI
cargo run -p relay -- --help

cd app && flutter run -d windows
cd app && flutter build apk --debug --target-platform android-arm64
cd app && flutter_rust_bridge_codegen generate   # after changes in app/rust/src/api

cargo run --release -p relay --example load -- --devices 500 --secs 30   # load test
```

Release build (APK and Windows installer, version taken from `app/pubspec.yaml`):

```powershell
powershell -ExecutionPolicy Bypass -File tools\build-release.ps1
```

## Deploy

Ansible runs from WSL. Copy `infra/ansible/inventory.example.ini` to `inventory.ini` (it is gitignored) and fill in your host.

```sh
ansible-playbook -i infra/ansible/inventory.ini infra/ansible/bootstrap.yml   # fresh VPS, as root
ansible-playbook -i infra/ansible/inventory.ini infra/ansible/deploy.yml --skip-tags firewall
ansible-playbook -i infra/ansible/inventory.ini infra/ansible/deploy.yml --tags firewall
```

Server secrets (the relay's signing key, the TURN secret) are generated on the server and never leave it; the attachment key exists only in the relay's memory. Later rollouts of the manifest or a release use `--tags manifest` and `--tags release`; botatoadmin runs them for you.
