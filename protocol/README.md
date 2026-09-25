# botato protocol v1

Normative spec. Reference implementation: `crates/proto`. Test vectors: `vectors/*.json`
(regenerate only on an intentional format change: `UPDATE_VECTORS=1 cargo test -p proto`).

## 1. Encoding of signed objects

| Type | Encoding |
|---|---|
| `u8`, `u16`, `u64` | unsigned, big-endian |
| `id` | 16 raw bytes (random; user, device and message ids share one namespace; all-zero is invalid) |
| `str` | `u16` byte length + UTF-8, ≤1024 bytes |
| `[N]` | N raw bytes |

- The first byte of every body is its format version (`1`). Unknown version, truncation or trailing bytes → reject.
- Signature: `sig = Ed25519(key, ctx || body)`, verified with strict verification. Signed object = `body || sig` (64 bytes).
- Contexts (ASCII, NUL-terminated):

| ctx | Used for |
|---|---|
| `botato/v1/cert\0` | certificate |
| `botato/v1/manifest\0` | manifest |
| `botato/v1/auth-server\0` | server proof in the handshake |
| `botato/v1/auth-client\0` | device proof in the handshake |

## 2. Certificate (one per user, signed by the admin root key)

```
u8   format = 1
id   user_id
str  name            1..64 bytes
u64  issued_at       unix seconds
u64  expires_at      > issued_at
u8   n_devices       1..8
n × { id device_id  (≠ user_id, unique within the cert)
      [32] sign_pub  Ed25519
      [32] enc_pub   X25519 }
```

Valid at `now` iff the signature verifies, `issued_at ≤ now + 300` and `now < expires_at`.
Among valid certs of one `user_id` the one with the greatest `issued_at` wins.
A device is removed by issuing a newer cert without it **and** listing its `device_id` in the manifest's `revoked`.

## 3. Manifest (signed by the admin root key)

```
u8   format = 1
u64  version         strictly increasing
u64  issued_at
u8   n_servers       1..16
n × { str url        e.g. wss://relay.example.org/ws
      str ip_fallback   may be empty
      [32] server_pub }
u8   n_stun          ≤64,  n × str
u8   n_nostr         ≤64,  n × str
u16  n_revoked       ≤4096, n × id     user ids and/or device ids
```

A client accepts a manifest only with a valid signature and `version` greater than the current one.
A device is revoked if its `device_id` or its `user_id` is listed.
`GET /manifest` returns the raw signed manifest (`application/octet-stream`), 404 if the server has none.

## 3a. Enrollment request (device → admin)

```
u8   format = 1
id   device_id       random, chosen by the device
[32] sign_pub
[32] enc_pub
str  name            suggested display name, may be empty, ≤64 bytes
```
Signed by the device itself (`ctx = botato/v1/enroll\0`, key = `sign_pub`) as proof of possession.

Text form for QR codes and copy-paste: `botato-req1:` + base64url without padding.
The admin answers with `botato-cert1:` + base64url(cert). Parsers ignore whitespace.

**Fingerprint.** The request travels over an untrusted channel, so the admin and the user compare
the fingerprint by voice or in person before issuing a cert:
`h = SHA-256("botato/v1/fingerprint\0" || device_id || sign_pub || enc_pub)`; take `h[0..20]` as four
5-byte big-endian integers, each `mod 100000`, zero-padded to 5 digits, joined by spaces (`12345 67890 …`).

## 3b. Release (signed by the admin root key)

App updates are served by the relay host (`GET /releases/release.bin` and the files next to it) but
trusted only through this signature. Vector: `vectors/release.json`.

```
body = u8 format(1) | str version | u64 build | u64 issued_at
     | u8 n_files (1..8) | n × (str platform | str file | u64 size | [32] sha256)
sig  = Ed25519(root, "botato/v1/release\0" || body)
```

- `build` is monotonic; a client offers the update only if it is higher than its own build.
- `platform`: `android-arm64`, `windows-x64`. `file` is a bare file name (no `/`, `\`, `..`), fetched from
  `/releases/<file>`.
- The client downloads the file, checks length and SHA-256 against the signed entry, and only then hands
  it to the OS installer. A server can withhold or delay updates, not substitute them.

## 4. Transport

WebSocket at `/ws`, text frames only, one JSON object per frame with a string discriminator `t`.
Binary fields: standard base64 with padding. Ids: 32 lowercase hex chars. Unknown fields → error.
Frames are not signed; anything that needs integrity is signed inside blobs.

### 4.1 Handshake (must finish within 10 s)

```
C→S {"t":"hello","v":1,"nonce":b64(cn)}                                cn: 32 random bytes
S→C {"t":"challenge","nonce":b64(sn),"server_pub":b64,"sig":b64}       sig = Ed25519(server, AUTH_SERVER || cn || sn)
```
The client verifies that `server_pub` is listed in its manifest and that `sig` is valid; otherwise it disconnects
without sending anything else (the certificate is never revealed to an unverified server).
```
C→S {"t":"auth","cert":b64,"device_id":hex,"sig":b64}                  sig = Ed25519(device, AUTH_CLIENT || sn || server_pub || cn)
S→C {"t":"ready","user_id":hex,"device_id":hex}
```
The server checks: cert valid (§2), `device_id` in the cert, `sig` by that device's `sign_pub`, neither id revoked,
and that it has not seen a newer cert of the same user that omits this device. A new session of the same
`device_id` closes the previous one (4002). `ready` is always the first frame after `challenge`.

### 4.2 Frames after `ready`

| Direction | Frame | Notes |
|---|---|---|
| C→S | `send {to, id, blob}` | `to` = device id; `blob` 1..65536 bytes. Stored ≤24 h. Same `(to, id)` again is a no-op success |
| S→C | `stored {id}` | |
| S→C | `deliver {id, from, blob}` | in store order; repeated on every connection until acked |
| C→S | `ack {id}` | deletes the message from this device's mailbox; no reply |
| C→S | `signal {to, blob}` | not stored; if `to` is offline it is held in RAM ≤60 s and a wake-up push is sent |
| S→C | `signaled {to, online}` | `online=false` → held |
| S→C | `signal {from, blob}` | |
| C→S | `directory {}` | |
| S→C | `directory {certs: [b64]}` | newest known cert of every non-revoked user; clients must verify each |
| C→S | `turn {}` | |
| S→C | `turn {urls, username, credential, ttl}` | coturn REST credentials: `username` = expiry unix time, `credential` = base64(HMAC-SHA1(secret, username)) |
| C→S | `push_token {platform, token}` | `platform` ∈ `fcm`, `apns_voip`; token ≤4096 bytes; kept in RAM, resend after every connect |
| C→S | `upload {id, data}` | encrypted attachment (§6.3), 1..2 MiB; `id` random, chosen by the sender. Kept 24 h. Same `id` again is a no-op success |
| S→C | `uploaded {id}` | |
| C→S | `download {id}` | any authenticated device that knows the `id` |
| S→C | `file {id, data}` | or `error {ref: "download", id, code: "not_found"}` |
| C→S | `presence {devices}` | ≤256 device ids |
| S→C | `presence {online}` | the queried devices that have a session now (a phone asleep is offline but still reachable by push) |
| S→C | `ok {ref}` | |
| S→C | `error {ref, id?, code}` | `ref` = `t` of the offending frame (empty if unparsable) |

`from` set by the server is a routing hint only; the authenticated sender is inside the blob.

Error codes: `bad_frame`, `too_large`, `quota`, `rate_limited`, `revoked`, `unavailable`, `internal`, `not_found`.

Close codes: `4000` protocol error, `4001` auth failed, `4002` replaced by a newer session of the same device,
`4003` revoked, `4004` handshake timeout, `1001` server shutdown.

### 4.3 Server limits (defaults)

| Limit | Value |
|---|---|
| Mailbox TTL | 24 h |
| Pending per sender | 1000 messages / 16 MiB |
| Pending per recipient device | 2000 messages / 32 MiB |
| Rate | token bucket, 20 frames/s, burst 100, per connection |
| Concurrent unfinished handshakes | 16 (each may buffer one message up to the attachment size) |
| Held signals per device | 32, 60 s |
| Keepalive | server ping every 30 s; no inbound traffic for 75 s → close |
| Wake-up push debounce | messages 30 s, calls 5 s |
| Attachments | ≤2 MiB each, 24 h; per uploading device 64 MiB / 200 files; 1 GiB in total |

Attachments are the one thing the relay keeps on disk (too big for RAM). Each file is encrypted again
with a key that exists only in the relay's memory, and the directory is wiped at start. The index is in RAM.

## 5. Device secret (client storage, not on the wire)

`u8 format(1) | id device_id | [32] Ed25519 seed | [32] X25519 secret` — kept in platform secure storage.

## 6. E2E envelope (`blob` of `send` and `signal`)

Everything between devices is sealed to the recipient device's `enc_pub` and signed by the sender device.
Vector: `vectors/envelope.json`.

```
inner  = u8 format(1) | u8 kind | id sender_device | u64 sent_at | u32 len | payload
sig    = Ed25519(sender.sign, "botato/v1/envelope\0" || recipient_device || eph_pub || inner)
shared = X25519(eph_secret, recipient.enc_pub)          all-zero (non-contributory) → reject
key    = HKDF-SHA256(salt = eph_pub || recipient.enc_pub, ikm = shared, info = "botato/v1/envelope-key", L = 32)
header = u8 version(1) | [32] eph_pub | [24] nonce
blob   = header || XChaCha20-Poly1305(key, nonce, plaintext = inner || sig, AAD = header || recipient_device)
```

Receiving: decrypt with own X25519 secret → take `sender_device` from `inner` → find it in a verified,
non-revoked certificate → verify `sig` with that device's `sign_pub`. Anything else is dropped silently.
The signature covers the recipient and `eph_pub`, so a recipient cannot re-address a signed envelope.
Payload ≤ 65280 bytes.

| kind | Meaning | payload |
|---|---|---|
| 1 | call signaling | JSON, see below |
| 2 | message | JSON, see §6.2 |
| 3 | E2E receipt | JSON, see §6.2 |
| 4 | message update (edit, delete, reaction) | JSON, see §6.2 |

### 6.1 Call signaling payload (kind 1)

UTF-8 JSON object. Receivers drop signals with `|now − sent_at| > 120 s`.

| `type` | Fields | Meaning |
|---|---|---|
| `offer` | `call`, `sdp`, `video` (bool), `vcodec`? (e.g. `H264`) | start a call; sent to every device of the callee. `vcodec`: the caller's preferred video codec; the callee puts it first in its answer if it has it |
| `answer` | `call`, `sdp` | accepted on this device |
| `ice` | `call`, `candidate`, `mid`, `mline` | trickle ICE candidate |
| `hangup` | `call`, `reason` (`hangup`, `declined`, `busy`, `answered_elsewhere`, `failed`) | end or reject |
| `media` | `call`, `video` (bool), `screen` (bool) | what this side currently sends on its video track |
| `reoffer` | `call`, `sdp` | caller → callee: ICE restart after a network change (`iceRestart`) |
| `reanswer` | `call`, `sdp` | callee → caller: answer to `reoffer` |
| `ping` / `pong` | — | diagnostics: round trip through the relay, answered automatically |

Every call negotiates one audio and one video transceiver, both `sendrecv`, even for voice calls.
Camera, screen sharing, device changes and audio modes swap tracks (`replaceTrack`) and sender
parameters without renegotiation; `media` tells the peer what to render. Opus fmtp at setup:
`stereo=1; sprop-stereo=1; useinbandfec=1; maxaveragebitrate=128000; maxplaybackrate=48000`
(the actual audio bitrate per mode is set with `setParameters`). On ICE `disconnected`/`failed` the
caller sends `reoffer` (repeated every 6 s until media flows); both sides give up after 40 s.

`call` is a random 128-bit id (hex). SDP needs no separate signature: the envelope signature authenticates
it, and WebRTC checks the peer's DTLS certificate against the fingerprint in that SDP.

### 6.2 Messages (kind 2) and receipts (kind 3)

Sent through the mailbox (`send`), never through `signal`.

```
kind 2: {"msg": hex id, "to": hex user id, "text": string, "photo"?: Photo, "voice"?: Voice, "reply"?: Reply, "fwd"?: string}
        text ≤ 16 KiB after trimming; fwd = display name of the original author (≤ 64 chars)
Photo:  {"file": hex id, "key": b64 32 bytes, "size": u32, "w": u32, "h": u32, "thumb": b64 JPEG ≤ 8 KiB}
Voice:  {"file": hex id, "key": b64 32 bytes, "size": u32, "ms": u32 (1..600000), "wave": b64 ≤ 128 bytes}
Reply:  {"msg": hex id, "from": hex user id, "text": string ≤ 200 chars}      quoted snapshot
kind 4: {"id": hex id, "msg": hex id, "to": hex user id, "op": "edit" | "delete" | "react",
         "text"?: string (edit), "emoji"?: string ≤ 32 bytes (react; "" removes)}
kind 3: {"msg": hex id}
```

- `msg` is the logical message id, the same in every copy; each copy has its own random transport `id`.
- The sender seals one copy per device of the recipient **and** per own other device. `to` is the
  conversation partner as the sender sees it, so the sender's own devices file the copy under `to`.
- Every copy stays in the sender's outbox until that device's receipt arrives; the whole outbox is
  re-sent on every connection (the relay deduplicates by `(to, id)`). A message is *delivered* when a
  device of the recipient user sent a receipt. Copies older than 24 h are dropped; a message without
  any recipient receipt by then is *failed*.
- The receiver deduplicates by `msg`, answers **every** copy (duplicates too) with a receipt, and only
  then acks the delivery. A delivery from an unknown device is not acked: the client fetches the
  directory and processes it afterwards.
- Receipts are fire-and-forget: they leave the outbox once a relay stored them. If one is lost, the
  sender re-sends the message on its next connection and gets a new receipt.
- Ordering: by the sender's `sent_at` (seconds), ties by local arrival order.
- `text` may be empty only when `photo` or `voice` is present (then it is the caption). At most one of them.
- `voice`: AAC-LC in an MP4/M4A container as recorded, not re-encoded; `ms` is the duration, `wave` one
  loudness byte per bar for the bubble. Uploaded, fetched and checked exactly like a photo file (§6.3).
- `reply` carries a snapshot of the quoted message, so it renders even where the original is gone.
- kind 4 (`id` = its own logical id) travels exactly like kind 2: one copy per device of the partner and
  per own other device, kept in the outbox until receipted (receipt `msg` = the update's `id`).
  `edit` is applied only when the sender's user wrote `msg`; `delete` may come from either side of the
  conversation and removes the message without a trace; `react` sets the sender user's single reaction
  on `msg`. An update for a message this device doesn't have is receipted and dropped.
  Edits apply in `sent_at` order (an older edit never overwrites a newer one); a delete is final.

### 6.3 Attachments

The photo itself is uploaded separately (`upload`), encrypted with a fresh random key that travels
only in the kind-2 payload. Vector: `vectors/file.json`.

```
blob = u8 version(1) | [24] nonce | XChaCha20-Poly1305(key, nonce, plaintext, AAD = "botato/v1/file\0" || file_id)
```

- Photos are re-encoded by the sender before sealing, metadata dropped (EXIF orientation applied to the
  pixels): JPEG with the longest side ≤ 1600 px; a PNG source (screenshots, transparency) may stay PNG
  with the longest side ≤ 2560 px if it fits the size limit. Receivers tell JPEG and PNG apart by the magic
  bytes. `thumb` is always a JPEG ≤ 64 px (transparency over white), shown until the file arrives.
- The sender uploads before sending the message copies, on the same connection (the relay handles a
  connection's frames in order). It re-uploads on every connection while some copy of the message is
  still in its outbox: a restarted relay has lost its files.
- A receiver downloads when the message arrives and retries on later connections; `not_found` more than
  24 h after `sent_at` means the photo is gone. `size` is the blob length; a different length, or a
  blob that does not open with `key`, is rejected.

## 7. Not yet specified

History sync to a newly added device (not planned for v1), attachments other than photos.
