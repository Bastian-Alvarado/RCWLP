# RCWLP wire protocol (v2)

v2 adds the controller type to CONNECT and motion data to INPUT. v1 and v2 don't talk to
each other; update the host and the app together.

Everything is UDP on one host port (default **47391**). All integers are big-endian.
Implementations: `host/src/protocol.rs` + `host/src/crypto.rs` (PC) and
`android/.../Protocol.kt` + `Crypto.kt` (phone). Change both together.

## Header

Every packet starts with 6 bytes:

| bytes | field |
|---|---|
| 4 | magic `RCWL` |
| 1 | version `2` |
| 1 | kind |

Strings are `u8 length` + UTF-8 bytes (at most 64). `mac` is the first 16 bytes of
HMAC-SHA256 over every byte of the packet before it.

## Packets

| kind | name | direction | body after header |
|---|---|---|---|
| 0x01 | DISCOVER | phone → broadcast | — |
| 0x02 | DISCOVER_REPLY | host → phone | host_id[16], name |
| 0x10 | PAIR_HELLO | phone → host | client_id[16], client_pub[65], name |
| 0x11 | PAIR_CHALLENGE | host → phone | host_id[16], host_pub[65] |
| 0x12 | PAIR_RESULT | host → phone | status u8 (1 ok, 0 rejected), mac[16] = MAC(pair_key, "rcwlp-pair-ok") |
| 0x20 | CONNECT | phone → host | client_id[16], cnonce[16], pad_type u8 (0 Xbox 360, 1 DualShock 4), mac (pair_key) |
| 0x21 | WELCOME | host → phone | hnonce[16], mac (session_key) |
| 0x22 | CONNECT_REJECT | host → phone | reason u8 (1 = not paired). Unauthenticated. |
| 0x30 | INPUT | phone → host | client_id[16], seq u32, buttons u16, lt u8, rt u8, lx i16, ly i16, rx i16, ry i16, gyro x/y/z i16, accel x/y/z i16, mac (session_key) |
| 0x31 | STATUS | host → phone | seq u32, large_motor u8, small_motor u8, player u8, mac (session_key) |
| 0x40 | DISCONNECT | phone → host | client_id[16], seq u32, mac (session_key) |

`buttons`, triggers and sticks use XInput's `XINPUT_GAMEPAD` layout and bit values
(A = 0x1000, LB = 0x0100, dpad up = 0x0001, ...). Stick Y is positive up. For a
DualShock 4 the host translates them (A → cross, Back → share, Guide → PS, ...).

Motion uses DualShock 4 units: gyro 16 per deg/s, accel 8192 per g. Axes are the
controller's: X right, Y up out of the face (the phone screen), Z toward the player (down
the screen as it's held). Accel reads +8192 on the axis pointing up at rest. Xbox mode
ignores motion, and the phone sends zeros there.

## Keys

```
pair:    shared   = ECDH-P256(x coordinate, 32 bytes)
         pair_key = HMAC(shared, "rcwlp-pair-v1" || client_id || host_id || client_pub || host_pub)
         code     = u32(HMAC(pair_key, "rcwlp-code")[0..4]) mod 1_000_000
session: session_key = HMAC(pair_key, "rcwlp-session" || cnonce || hnonce)
```

Public keys are uncompressed SEC1 points (`0x04 || x || y`).

## Flows

**Pairing.** The phone sends PAIR_HELLO every second. The host replies with
PAIR_CHALLENGE and shows the 6-digit code; the phone derives the same key and shows the
same code. The user types `y` on the host if the codes match, and the host's next reply is
PAIR_RESULT. Comparing the codes is what protects against a man in the middle. Without
that check, anyone on the LAN could pose as the PC.

**Connecting.** The phone sends CONNECT with a fresh `cnonce` every 500 ms until it gets a
WELCOME it can verify. The host keeps the new session *pending* and only switches to it on
the first valid INPUT. That way a replayed CONNECT can't kick a phone that's in use.

**Streaming.** The phone sends INPUT on every state change (at most every 4 ms) and at
least every 50 ms; each packet carries the full state, so loss is harmless. If a phone
reconnects asking for a different pad_type, the host replaces its virtual controller. `seq` must strictly increase within a
session. The host sends STATUS on rumble changes and every 250 ms. That doubles as its
heartbeat.

**Timeouts.** The host releases all buttons after 1 s with no input and unplugs the
virtual controller after 15 s. The phone reconnects with a new session if it hears no
STATUS for 3 s.
