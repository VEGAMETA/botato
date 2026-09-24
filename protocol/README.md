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
| S→C | `ok {ref}` | |
| S→C | `error {ref, id?, code}` | `ref` = `t` of the offending frame (empty if unparsable) |

`from` set by the server is a routing hint only; the authenticated sender is inside the blob.

Error codes: `bad_frame`, `too_large`, `quota`, `rate_limited`, `revoked`, `unavailable`, `internal`.

Close codes: `4000` protocol error, `4001` auth failed, `4002` replaced by a newer session of the same device,
`4003` revoked, `4004` handshake timeout, `1001` server shutdown.

### 4.3 Server limits (defaults)

| Limit | Value |
|---|---|
| Mailbox TTL | 24 h |
| Pending per sender | 1000 messages / 16 MiB |
| Pending per recipient device | 2000 messages / 32 MiB |
| Rate | token bucket, 20 frames/s, burst 100, per connection |
| Concurrent unfinished handshakes | 64 |
| Held signals per device | 32, 60 s |
| Keepalive | server ping every 30 s; no inbound traffic for 75 s → close |
| Wake-up push debounce | messages 30 s, calls 5 s |

## 5. Not yet specified

Message blob envelope (X25519 → HKDF-SHA256 → XChaCha20-Poly1305 with inner sender signature),
E2E receipts, signed SDP. They will be added here with vectors before clients use them.
