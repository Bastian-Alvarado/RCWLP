//! Virtual controllers backed by the ViGEmBus driver: Xbox 360, or DualShock 4 with motion.

use std::thread::JoinHandle;
use std::time::Instant;

use vigem_client::{Client, DualShock4Wired, TargetId, XButtons, XGamepad, Xbox360Wired};

use crate::protocol::{PadKind, PadState};

const ERROR_NO_MORE_ITEMS: u32 = 259;

enum Target {
    Xbox360(Xbox360Wired<Client>),
    DualShock4 { target: DualShock4Wired<Client>, started: Instant, counter: u8 },
}

pub struct Pad {
    // Dropping the target unplugs it, which also ends the notification thread.
    target: Target,
    _notifications: JoinHandle<()>,
    /// 1-based XInput player number for Xbox pads, 0 when unknown or not applicable.
    pub player: u8,
    pub kind: PadKind,
}

impl Pad {
    /// Plugs in a virtual controller. `on_rumble(large, small)` runs on the driver's
    /// notification thread whenever a game changes the motors.
    pub fn plug(kind: PadKind, mut on_rumble: impl FnMut(u8, u8) + Send + 'static) -> Result<Pad, vigem_client::Error> {
        match kind {
            PadKind::Xbox360 => {
                let mut target = Xbox360Wired::new(Client::connect()?, TargetId::XBOX360_WIRED);
                target.plugin()?;
                target.wait_ready()?;
                let player = target.get_user_index().map(|i| i as u8 + 1).unwrap_or(0);
                let notifications = target
                    .request_notification()?
                    .spawn_thread(move |_, n| on_rumble(n.large_motor, n.small_motor));
                Ok(Pad { target: Target::Xbox360(target), _notifications: notifications, player, kind })
            }
            PadKind::DualShock4 => {
                let mut target = DualShock4Wired::new(Client::connect()?, TargetId::DUALSHOCK4_WIRED);
                target.plugin()?;
                target.wait_ready()?;
                let notifications =
                    target.request_notification()?.spawn_thread(move |n| on_rumble(n.large_motor, n.small_motor));
                let target = Target::DualShock4 { target, started: Instant::now(), counter: 0 };
                Ok(Pad { target, _notifications: notifications, player: 0, kind })
            }
        }
    }

    pub fn update(&mut self, s: &PadState) {
        let result = match &mut self.target {
            Target::Xbox360(target) => target.update(&XGamepad {
                buttons: XButtons(s.buttons),
                left_trigger: s.left_trigger,
                right_trigger: s.right_trigger,
                thumb_lx: s.thumb_lx,
                thumb_ly: s.thumb_ly,
                thumb_rx: s.thumb_rx,
                thumb_ry: s.thumb_ry,
            }),
            Target::DualShock4 { target, started, counter } => {
                *counter = counter.wrapping_add(1);
                // DS4 timestamps tick every 5.33 µs; Steam uses them to integrate the gyro.
                let ticks = (started.elapsed().as_micros() * 3 / 16) as u16;
                target.update_ex(&ds4_report(s, *counter, ticks))
            }
        };
        match result {
            // DS4: Windows hasn't polled for the next report yet (common right after plugging in).
            // The phone resends its full state within 50 ms, so dropping this one is harmless.
            Ok(()) | Err(vigem_client::Error::WinError(ERROR_NO_MORE_ITEMS)) => {}
            Err(e) => eprintln!("Virtual controller update failed: {e}"),
        }
    }
}

mod xb {
    pub const UP: u16 = 0x0001;
    pub const DOWN: u16 = 0x0002;
    pub const LEFT: u16 = 0x0004;
    pub const RIGHT: u16 = 0x0008;
    pub const START: u16 = 0x0010;
    pub const BACK: u16 = 0x0020;
    pub const LTHUMB: u16 = 0x0040;
    pub const RTHUMB: u16 = 0x0080;
    pub const LB: u16 = 0x0100;
    pub const RB: u16 = 0x0200;
    pub const GUIDE: u16 = 0x0400;
    pub const A: u16 = 0x1000;
    pub const B: u16 = 0x2000;
    pub const X: u16 = 0x4000;
    pub const Y: u16 = 0x8000;
}

/// Builds the 63-byte DS4 USB input report (without the report ID) from an Xbox-style state.
/// Offsets follow DS4_REPORT_EX in ViGEmClient's Common.h.
fn ds4_report(s: &PadState, counter: u8, timestamp: u16) -> [u8; 63] {
    let b = s.buttons;
    let has = |bit: u16| b & bit != 0;

    let dpad: u16 = match (has(xb::UP), has(xb::DOWN), has(xb::LEFT), has(xb::RIGHT)) {
        (true, false, false, false) => 0,
        (true, false, false, true) => 1,
        (false, false, false, true) => 2,
        (false, true, false, true) => 3,
        (false, true, false, false) => 4,
        (false, true, true, false) => 5,
        (false, false, true, false) => 6,
        (true, false, true, false) => 7,
        _ => 8, // none, or an impossible combination
    };
    let map = [
        (xb::X, 1 << 4),  // square
        (xb::A, 1 << 5),  // cross
        (xb::B, 1 << 6),  // circle
        (xb::Y, 1 << 7),  // triangle
        (xb::LB, 1 << 8), // L1
        (xb::RB, 1 << 9), // R1
        (xb::BACK, 1 << 12),
        (xb::START, 1 << 13),
        (xb::LTHUMB, 1 << 14),
        (xb::RTHUMB, 1 << 15),
    ];
    let mut buttons = dpad;
    for (x, d) in map {
        if has(x) {
            buttons |= d;
        }
    }
    if s.left_trigger > 0 {
        buttons |= 1 << 10;
    }
    if s.right_trigger > 0 {
        buttons |= 1 << 11;
    }

    let mut r = [0u8; 63];
    r[0] = stick(s.thumb_lx);
    r[1] = stick(s.thumb_ly.saturating_neg()); // DS4 Y grows downward
    r[2] = stick(s.thumb_rx);
    r[3] = stick(s.thumb_ry.saturating_neg());
    r[4..6].copy_from_slice(&buttons.to_le_bytes());
    r[6] = (counter & 0x3F) << 2 | has(xb::GUIDE) as u8;
    r[7] = s.left_trigger;
    r[8] = s.right_trigger;
    r[9..11].copy_from_slice(&timestamp.to_le_bytes());
    for i in 0..3 {
        r[12 + i * 2..14 + i * 2].copy_from_slice(&s.gyro[i].to_le_bytes());
        r[18 + i * 2..20 + i * 2].copy_from_slice(&s.accel[i].to_le_bytes());
    }
    r[29] = 0x1B; // battery: cable connected, full
    r[32] = 1; // one touch packet...
    r[34] = 0x80; // ...with both fingers up (the flag is active low), so no phantom touches
    r[38] = 0x80;
    r[43] = 0x80; // previous touch packets too
    r[47] = 0x80;
    r[52] = 0x80;
    r[56] = 0x80;
    r
}

fn stick(v: i16) -> u8 {
    ((v as i32 + 32768) >> 8) as u8
}

/// Fails fast with a useful message if the driver is missing.
pub fn check_driver() -> Result<(), vigem_client::Error> {
    Client::connect().map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neutral_report() {
        let r = ds4_report(&PadState::default(), 0, 0);
        assert_eq!(&r[0..4], &[128, 128, 128, 128]);
        assert_eq!(u16::from_le_bytes([r[4], r[5]]), 8, "dpad none, nothing pressed");
        assert_eq!(r[6], 0);
    }

    #[test]
    fn buttons_sticks_and_motion() {
        let s = PadState {
            buttons: xb::A | xb::UP | xb::RIGHT | xb::GUIDE | xb::RB,
            left_trigger: 200,
            thumb_lx: i16::MAX,
            thumb_ly: i16::MAX, // pushed up
            gyro: [-16, 32, 1600],
            accel: [0, 8192, -1],
            ..Default::default()
        };
        let r = ds4_report(&s, 5, 0x1234);
        assert_eq!(r[0], 255);
        assert_eq!(r[1], 0, "stick up is 0 on a DS4");
        let buttons = u16::from_le_bytes([r[4], r[5]]);
        assert_eq!(buttons & 0xF, 1, "north-east");
        assert_ne!(buttons & 1 << 5, 0, "cross");
        assert_ne!(buttons & 1 << 9, 0, "R1");
        assert_ne!(buttons & 1 << 10, 0, "L2 digital");
        assert_eq!(r[6], 5 << 2 | 1, "counter and PS button");
        assert_eq!(r[7], 200);
        assert_eq!(&r[9..11], &[0x34, 0x12]);
        assert_eq!(i16::from_le_bytes([r[12], r[13]]), -16);
        assert_eq!(i16::from_le_bytes([r[16], r[17]]), 1600);
        assert_eq!(i16::from_le_bytes([r[20], r[21]]), 8192);
        assert_eq!(i16::from_le_bytes([r[22], r[23]]), -1);
    }
}
