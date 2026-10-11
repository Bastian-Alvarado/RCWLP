//! Shows what games read from the virtual Xbox controllers through XInput: buttons, triggers
//! and sticks, printed whenever they change. Handy to check a phone's controller end to end.
//!
//!   cargo run --example xinput_watch -- [seconds]

use std::time::{Duration, Instant};

use winapi::um::xinput::{XINPUT_STATE, XInputGetState};

const BUTTONS: [(u16, &str); 14] = [
    (0x0001, "up"),
    (0x0002, "down"),
    (0x0004, "left"),
    (0x0008, "right"),
    (0x0010, "Start"),
    (0x0020, "Back"),
    (0x0040, "LS"),
    (0x0080, "RS"),
    (0x0100, "LB"),
    (0x0200, "RB"),
    (0x1000, "A"),
    (0x2000, "B"),
    (0x4000, "X"),
    (0x8000, "Y"),
];

fn main() {
    let seconds: u64 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(20);
    println!("Watching XInput controllers for {seconds} s");
    let deadline = Instant::now() + Duration::from_secs(seconds);
    let mut last: [Option<String>; 4] = Default::default();
    while Instant::now() < deadline {
        for (player, last) in last.iter_mut().enumerate() {
            // Safety: XInputGetState fills the struct it's given.
            let mut state: XINPUT_STATE = unsafe { std::mem::zeroed() };
            let connected = unsafe { XInputGetState(player as u32, &mut state) } == 0;
            let now = if connected { describe(&state) } else { "not connected".into() };
            if last.as_deref() != Some(now.as_str()) {
                println!("player {}: {now}", player + 1);
                *last = Some(now);
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Everything in words. Sticks only show when pushed past a quarter.
fn describe(s: &XINPUT_STATE) -> String {
    let g = &s.Gamepad;
    let mut parts: Vec<String> = BUTTONS.iter().filter(|(bit, _)| g.wButtons & bit != 0).map(|(_, n)| n.to_string()).collect();
    if g.wButtons & 0x0400 != 0 {
        parts.push("Guide".into());
    }
    for (v, name) in [(g.bLeftTrigger, "LT"), (g.bRightTrigger, "RT")] {
        if v > 0 {
            parts.push(format!("{name} {v}"));
        }
    }
    for ((x, y), name) in [((g.sThumbLX, g.sThumbLY), "left stick"), ((g.sThumbRX, g.sThumbRY), "right stick")] {
        if x.unsigned_abs() > 8192 || y.unsigned_abs() > 8192 {
            parts.push(format!("{name} ({:+.2}, {:+.2})", x as f32 / 32767.0, y as f32 / 32767.0));
        }
    }
    if parts.is_empty() { "nothing pressed".into() } else { parts.join(", ") }
}
