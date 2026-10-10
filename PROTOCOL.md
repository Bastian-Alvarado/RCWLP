# RCWLP wire protocol (v4)

v4 changes streaming: INPUT carries up to four controllers per phone, touchpad data, the
phone's battery and its measured latency; STATUS acknowledges input (for latency) and
carries rumble and lightbar per controller; UNPAIR lets the phone remove itself from the
host. v3 changed pairing: PAIR_HELLO commits to the phone's key and PAIR_REVEAL sends it.
Versions don't talk to each other; update the host and the app together. Pairings made
with v2 or v3 keep working, since the keys are derived the same way.

Discovery is the exception, so a mismatch can be explained instead of looking like "no PC
found". The host answers DISCOVER of any version, always in its own version, and logs that
the phone is outdated or newer. The phone sends DISCOVER in its own version and in v2, and
reads DISCOVER_REPLY of any version. A reply in another version means "update one side",
and the version byte says which.

Everything is UDP on one host port (default **47391**). All integers are big-endian.
Implementations: `host/src/protocol.rs` + `host/src/crypto.rs` (PC) and
`android/.../Protocol.kt` + `Crypto.kt` (phone). Change both together, and keep
`protocol-vectors.txt` passing on both sides (`cargo test`, `./gradlew testDebugUnitTest`).

## Header

Every packet starts with 6 bytes:

| bytes | field |
|---|---|
| 4 | magic `RCWL` |
| 1 | version `4` |
| 1 | kind |

Strings are `u8 length` + UTF-8 bytes (at most 64). `mac` is the first 16 bytes of
HMAC-SHA256 over every byte of the packet before it.

## Packets

| kind | name | direction | body after header |
|---|---|---|---|
| 0x01 | DISCOVER | phone → broadcast | — |
| 0x02 | DISCOVER_REPLY | host → phone | host_id[16], name, host_version (string, from host 0.2.0 on; may be absent) |
| 0x10 | PAIR_HELLO | phone → host | client_id[16], commitment[32], name |
| 0x11 | PAIR_CHALLENGE | host → phone | host_id[16], host_pub[65] |
| 0x12 | PAIR_RESULT | host → phone | status u8 (1 ok, 0 rejected), mac[16] = MAC(pair_key, "rcwlp-pair-ok") |
| 0x13 | PAIR_REVEAL | phone → host | client_id[16], client_pub[65] |
| 0x20 | CONNECT | phone → host | client_id[16], cnonce[16], pad_type u8 (0 Xbox 360, 1 DualShock 4), mac (pair_key) |
| 0x21 | WELCOME | host → phone | hnonce[16], mac (session_key) |
| 0x22 | CONNECT_REJECT | host → phone | reason u8 (1 = not paired). Unauthenticated. |
| 0x30 | INPUT | phone → host | client_id[16], seq u32, rtt_ms u16, battery u8, pad_count u8, pad × pad_count, mac (session_key) |
| 0x31 | STATUS | host → phone | seq u32, ack_seq u32, ack_age_ms u16, pad_count u8, feedback × pad_count, mac (session_key) |
| 0x40 | DISCONNECT | phone → host | client_id[16], seq u32, mac (session_key) |
| 0x41 | UNPAIR | phone → host | client_id[16], mac (pair_key) |
| 0x42 | UNPAIRED | host → phone | client_id[16], mac (pair_key) |

INPUT fields:

- `rtt_ms`: the phone's latest round-trip time to the host (see Streaming), `0xFFFF` if unknown.
- `battery`: bit 7 set while charging; bits 0–6 the phone's charge in percent, `0x7F` if unknown.
- `pad`, 34 bytes: slot u8 (0–3), buttons u16, lt u8, rt u8, lx i16, ly i16, rx i16, ry i16,
  gyro x/y/z i16, accel x/y/z i16, touch u8, finger 0 x u16, y u16, finger 1 x u16, y u16.

STATUS `feedback`, 7 bytes: slot u8, large_motor u8, small_motor u8, player u8, lightbar
red u8, green u8, blue u8. `player` is the 1-based XInput player number, 0 if unknown.
The lightbar is the colour a game set on a DualShock 4, all zero otherwise.

`buttons`, triggers and sticks use XInput's `XINPUT_GAMEPAD` layout and bit values
(A = 0x1000, LB = 0x0100, dpad up = 0x0001, ...). Stick Y is positive up. Bit 0x0800, unused
by XInput, is the DualShock 4 touchpad click. For a DualShock 4 the host translates them
(A → cross, Back → share, Guide → PS, ...).

Motion uses DualShock 4 units: gyro 16 per deg/s, accel 8192 per g. Axes are the
controller's: X right, Y up out of the face (the phone screen), Z toward the player (down
the screen as it's held). Accel reads +8192 on the axis pointing up at rest. Xbox mode
ignores motion, and the phone sends zeros there. The phone's sensors belong to slot 0.

Touch uses the DualShock 4 touchpad's coordinates: x 0–1919, y 0–942. `touch` bit 0 means
finger 0 is down, bit 1 finger 1. Positions of fingers that are up are ignored. The host
numbers touches itself, as a DualShock 4 does.

## Keys

```
pair:    commitment = SHA-256("rcwlp-commit" || client_pub)
         shared     = ECDH-P256(x coordinate, 32 bytes)
         pair_key   = HMAC(shared, "rcwlp-pair-v1" || client_id || host_id || client_pub || host_pub)
         code       = u32(HMAC(pair_key, "rcwlp-code")[0..4]) mod 1_000_000
session: session_key = HMAC(pair_key, "rcwlp-session" || cnonce || hnonce)
```

Public keys are uncompressed SEC1 points (`0x04 || x || y`).

## Flows

**Pairing.** Both sides use a fresh key pair for every attempt.

1. The phone sends PAIR_HELLO every second until it gets PAIR_CHALLENGE, which carries the
   host's key. The host answers a repeated hello with the same challenge.
2. The phone derives the pair key, shows the 6-digit code, and sends PAIR_REVEAL every
   second until it gets PAIR_RESULT. The host checks the key against the hello's
   commitment and derives the same pair key.
3. The user types the phone's code on the host. The host accepts only if it matches its own
   code, and answers with PAIR_RESULT (also to any later REVEAL or HELLO of that attempt).

The code is what stops a man in the middle: without it, anyone on the LAN could pose as the
PC. The commitment keeps the code honest. A middleman has to pick the key it shows the
phone before it learns the phone's key, and the host picks its key after the middleman has
committed to one. Neither side can be searched for a matching code, so a middleman
succeeds with one chance in a million. Typing the code, rather than confirming with `y`,
makes sure it was actually compared.

The host handles one request at a time and expires it after 60 s. A hello from the same
phone with a different commitment (the user cancelled and retried) replaces the request,
but only once it's 3 s old, so a middleman can't cycle through requests looking for a match.

**Unpairing.** When the user forgets a PC on the phone, the phone sends UNPAIR a few times.
The host checks the MAC with the stored pair key, forgets the phone, unplugs its
controllers and answers UNPAIRED, signed with the same key so the phone knows it was the
real host. A replayed UNPAIR is harmless: the key it was made with is gone, and a new
pairing has a new key.

**Connecting.** The phone sends CONNECT with a fresh `cnonce` every 500 ms until it gets a
WELCOME it can verify. The host keeps the new session *pending* and only switches to it on
the first valid INPUT. That way a replayed CONNECT can't kick a phone that's in use.

The phone sends CONNECT to the host's saved address and, if discovery last saw it somewhere
else, there too. DISCOVER_REPLY isn't authenticated, so the phone only switches to (and
saves) the address that answers with a valid WELCOME. It believes CONNECT_REJECT only while
connecting, and only once every address it tried has sent one.

**Streaming.** The phone sends INPUT on every state change (at most every 4 ms) and at
least every 50 ms; each packet carries the full state of every controller, so loss is
harmless. `seq` must strictly increase within a session. A slot the host hasn't seen gets a
new virtual controller; a slot missing from INPUT for 2 s is unplugged. The phone always
sends slot 0, even with no controller attached. If a phone reconnects asking for a
different pad_type, the host replaces its virtual controllers.

The host sends STATUS on rumble or lightbar changes and every 250 ms; that doubles as its
heartbeat. `ack_seq` is the last INPUT it accepted and `ack_age_ms` how long ago that was,
so the phone gets the round trip as (now − when it sent `ack_seq`) − `ack_age_ms`, without
the wait for the next STATUS counting. The phone reports it back in `rtt_ms` so the host
can show it.

**Timeouts.** The host releases all buttons after 1 s with no input and unplugs the
virtual controllers after 15 s. The phone reconnects with a new session if it hears no
STATUS for 3 s.
