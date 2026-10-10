//! Wire format shared with the Android app. See PROTOCOL.md at the repo root.
//! All integers are big-endian.

pub const MAGIC: [u8; 4] = *b"RCWL";
pub const VERSION: u8 = 4;
pub const DEFAULT_PORT: u16 = 47391;
pub const MAC_LEN: usize = 16;
pub const PUB_LEN: usize = 65;
pub const MAX_NAME: usize = 64;
/// Controllers per phone.
pub const MAX_PADS: usize = 4;
/// Button bit for the DualShock 4 touchpad click; XInput leaves it unused.
pub const TOUCHPAD_CLICK: u16 = 0x0800;
pub const BATTERY_UNKNOWN: u8 = 0x7F;
pub const RTT_UNKNOWN: u16 = 0xFFFF;

pub type Id = [u8; 16];
pub type Nonce = [u8; 16];
pub type PubKey = [u8; PUB_LEN];
/// SHA-256 of the phone's pairing key, sent before the key itself (see PROTOCOL.md).
pub type Commitment = [u8; 32];

pub mod kind {
    pub const DISCOVER: u8 = 0x01;
    pub const DISCOVER_REPLY: u8 = 0x02;
    pub const PAIR_HELLO: u8 = 0x10;
    pub const PAIR_CHALLENGE: u8 = 0x11;
    pub const PAIR_RESULT: u8 = 0x12;
    pub const PAIR_REVEAL: u8 = 0x13;
    pub const CONNECT: u8 = 0x20;
    pub const WELCOME: u8 = 0x21;
    pub const CONNECT_REJECT: u8 = 0x22;
    pub const INPUT: u8 = 0x30;
    pub const STATUS: u8 = 0x31;
    pub const DISCONNECT: u8 = 0x40;
    pub const UNPAIR: u8 = 0x41;
    pub const UNPAIRED: u8 = 0x42;
}

/// Which virtual controller a phone asks for in CONNECT.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PadKind {
    Xbox360,
    /// Adds motion sensors, which Steam Input can use for gyro aiming.
    DualShock4,
}

impl PadKind {
    pub fn from_wire(v: u8) -> Option<PadKind> {
        match v {
            0 => Some(PadKind::Xbox360),
            1 => Some(PadKind::DualShock4),
            _ => None,
        }
    }
}

/// One controller snapshot. Buttons, triggers and sticks use XINPUT_GAMEPAD's layout, plus
/// [`TOUCHPAD_CLICK`]. Motion is in DualShock 4 units and axes: gyro 16 LSB per deg/s, accel
/// 8192 LSB per g, X right, Y up out of the controller's face, Z toward the player. Touches
/// are in DualShock 4 touchpad coordinates (x 0–1919, y 0–942).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct PadState {
    pub buttons: u16,
    pub left_trigger: u8,
    pub right_trigger: u8,
    pub thumb_lx: i16,
    pub thumb_ly: i16,
    pub thumb_rx: i16,
    pub thumb_ry: i16,
    pub gyro: [i16; 3],
    pub accel: [i16; 3],
    /// Up to two fingers on the touchpad, None while lifted.
    pub touch: [Option<(u16, u16)>; 2],
}

impl PadState {
    pub fn read(r: &mut Reader) -> Option<PadState> {
        let mut s = PadState {
            buttons: r.u16()?,
            left_trigger: r.u8()?,
            right_trigger: r.u8()?,
            thumb_lx: r.i16()?,
            thumb_ly: r.i16()?,
            thumb_rx: r.i16()?,
            thumb_ry: r.i16()?,
            gyro: [r.i16()?, r.i16()?, r.i16()?],
            accel: [r.i16()?, r.i16()?, r.i16()?],
            touch: [None; 2],
        };
        let down = r.u8()?;
        for (i, finger) in s.touch.iter_mut().enumerate() {
            let (x, y) = (r.u16()?, r.u16()?);
            if down & (1 << i) != 0 {
                *finger = Some((x, y));
            }
        }
        Some(s)
    }

    pub fn write(&self, mut w: Writer) -> Writer {
        w = w.u16(self.buttons).u8(self.left_trigger).u8(self.right_trigger);
        for v in [self.thumb_lx, self.thumb_ly, self.thumb_rx, self.thumb_ry].iter().chain(&self.gyro).chain(&self.accel) {
            w = w.i16(*v);
        }
        let down = self.touch.iter().enumerate().fold(0, |acc, (i, f)| acc | (f.is_some() as u8) << i);
        w = w.u8(down);
        for finger in self.touch {
            let (x, y) = finger.unwrap_or((0, 0));
            w = w.u16(x).u16(y);
        }
        w
    }
}

/// The body of an INPUT packet after client id and seq.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Input {
    /// The phone's round trip to the host in ms, [`RTT_UNKNOWN`] if it has none yet.
    pub rtt_ms: u16,
    /// Bit 7: charging. Bits 0–6: percent, [`BATTERY_UNKNOWN`] if unknown.
    pub battery: u8,
    /// (slot, state) for each controller on the phone. Slots are 0..MAX_PADS.
    pub pads: Vec<(u8, PadState)>,
}

impl Input {
    pub fn read(r: &mut Reader) -> Option<Input> {
        let rtt_ms = r.u16()?;
        let battery = r.u8()?;
        let count = r.u8()? as usize;
        if count > MAX_PADS {
            return None;
        }
        let mut pads = Vec::with_capacity(count);
        for _ in 0..count {
            let slot = r.u8()?;
            if slot as usize >= MAX_PADS || pads.iter().any(|(s, _)| *s == slot) {
                return None;
            }
            pads.push((slot, PadState::read(r)?));
        }
        Some(Input { rtt_ms, battery, pads })
    }

    pub fn write(&self, mut w: Writer) -> Writer {
        w = w.u16(self.rtt_ms).u8(self.battery).u8(self.pads.len() as u8);
        for (slot, state) in &self.pads {
            w = state.write(w.u8(*slot));
        }
        w
    }
}

/// Splits an authenticated packet into the signed part and its trailing MAC.
pub fn split_mac(buf: &[u8]) -> Option<(&[u8], &[u8])> {
    (buf.len() > MAC_LEN).then(|| buf.split_at(buf.len() - MAC_LEN))
}

pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Reader { buf, pos: 0 }
    }

    /// Checks magic and version, returns the packet kind.
    pub fn header(&mut self) -> Option<u8> {
        let (version, kind) = self.header_any()?;
        (version == VERSION).then_some(kind)
    }

    /// Checks only the magic and returns (version, kind). Discovery uses this to work across
    /// versions, so mismatched apps and hosts can say so instead of just not finding each other.
    pub fn header_any(&mut self) -> Option<(u8, u8)> {
        if self.array::<4>()? != MAGIC {
            return None;
        }
        Some((self.u8()?, self.u8()?))
    }

    pub fn array<const N: usize>(&mut self) -> Option<[u8; N]> {
        let bytes = self.buf.get(self.pos..self.pos + N)?;
        self.pos += N;
        bytes.try_into().ok()
    }

    pub fn u8(&mut self) -> Option<u8> {
        Some(self.array::<1>()?[0])
    }

    pub fn u16(&mut self) -> Option<u16> {
        Some(u16::from_be_bytes(self.array()?))
    }

    pub fn i16(&mut self) -> Option<i16> {
        Some(i16::from_be_bytes(self.array()?))
    }

    pub fn u32(&mut self) -> Option<u32> {
        Some(u32::from_be_bytes(self.array()?))
    }

    pub fn string(&mut self) -> Option<String> {
        let len = self.u8()? as usize;
        let bytes = self.buf.get(self.pos..self.pos + len)?;
        self.pos += len;
        let s = String::from_utf8_lossy(bytes);
        Some(s.chars().filter(|c| !c.is_control()).take(MAX_NAME).collect())
    }
}

pub struct Writer(Vec<u8>);

impl Writer {
    pub fn new(kind: u8) -> Self {
        Writer::with_version(VERSION, kind)
    }

    /// A packet of another protocol version. Only DISCOVER is meant to cross versions.
    pub fn with_version(version: u8, kind: u8) -> Self {
        let mut v = Vec::with_capacity(64);
        v.extend_from_slice(&MAGIC);
        v.push(version);
        v.push(kind);
        Writer(v)
    }

    pub fn u8(mut self, v: u8) -> Self {
        self.0.push(v);
        self
    }

    pub fn u16(mut self, v: u16) -> Self {
        self.0.extend_from_slice(&v.to_be_bytes());
        self
    }

    pub fn i16(mut self, v: i16) -> Self {
        self.0.extend_from_slice(&v.to_be_bytes());
        self
    }

    pub fn u32(mut self, v: u32) -> Self {
        self.0.extend_from_slice(&v.to_be_bytes());
        self
    }

    pub fn bytes(mut self, v: &[u8]) -> Self {
        self.0.extend_from_slice(v);
        self
    }

    pub fn string(self, s: &str) -> Self {
        let mut end = s.len().min(MAX_NAME);
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        self.u8(end as u8).bytes(&s.as_bytes()[..end])
    }

    /// Appends a MAC over everything written so far and returns the packet.
    pub fn sign(mut self, key: &[u8]) -> Vec<u8> {
        let mac = crate::crypto::mac16(key, &self.0);
        self.0.extend_from_slice(&mac);
        self.0
    }

    pub fn finish(self) -> Vec<u8> {
        self.0
    }
}
