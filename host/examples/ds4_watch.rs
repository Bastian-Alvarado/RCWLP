//! Shows what a game would read from the virtual DualShock 4: buttons, sticks, triggers,
//! touchpad and battery, printed whenever they change. Handy to check a phone end to end.
//!
//!   cargo run --example ds4_watch -- [seconds] [--motion] [--feedback R G B LARGE SMALL]
//!
//! `--motion` also prints the gyro and accelerometer once a second.
//! `--feedback` first sends what a game would: a lightbar colour and rumble (0-255 each).
//! The phone should take on the colour and vibrate.

use std::time::{Duration, Instant};

const SONY: u16 = 0x054C;
const DS4: u16 = 0x05C4;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let seconds: u64 = args.first().and_then(|s| s.parse().ok()).unwrap_or(20);
    let motion = args.iter().any(|a| a == "--motion");
    let feedback: Option<Vec<u8>> = args
        .iter()
        .position(|a| a == "--feedback")
        .map(|i| args[i + 1..i + 6].iter().map(|v| v.parse().expect("0-255")).collect());

    let api = hidapi::HidApi::new().expect("open HID");
    let info = api
        .device_list()
        .find(|d| d.vendor_id() == SONY && d.product_id() == DS4)
        .expect("no DualShock 4 plugged in; connect a phone in PS4 mode first");
    let device = info.open_device(&api).expect("open the DualShock 4");
    println!("Watching {} for {seconds} s", info.product_string().unwrap_or("DualShock 4"));

    if let Some(f) = feedback {
        // USB output report 0x05: flags (rumble, lightbar), then small motor, large motor, R, G, B.
        let mut report = [0u8; 32];
        report[..9].copy_from_slice(&[0x05, 0x03, 0x04, 0x00, f[4], f[3], f[0], f[1], f[2]]);
        device.write(&report).expect("send the output report");
        println!("Sent lightbar ({}, {}, {}) and rumble large {} small {}", f[0], f[1], f[2], f[3], f[4]);
    }

    let deadline = Instant::now() + Duration::from_secs(seconds);
    let mut next_motion = Instant::now();
    let mut buf = [0u8; 64];
    let mut last = String::new();
    while Instant::now() < deadline {
        let Ok(n) = device.read_timeout(&mut buf, 100) else { break };
        // USB input report 0x01; offsets below are the report's, report id included.
        if n < 43 || buf[0] != 0x01 {
            continue;
        }
        let now = describe(&buf);
        if now != last {
            println!("{now}");
            last = now;
        }
        if motion && Instant::now() >= next_motion {
            let i16_at = |at: usize| i16::from_le_bytes([buf[at], buf[at + 1]]);
            let gyro = [i16_at(13), i16_at(15), i16_at(17)].map(|g| g as f32 / 16.0);
            let accel = [i16_at(19), i16_at(21), i16_at(23)].map(|a| a as f32 / 8192.0);
            println!(
                "  gyro {:+.1} {:+.1} {:+.1} deg/s   accel {:+.2} {:+.2} {:+.2} g",
                gyro[0], gyro[1], gyro[2], accel[0], accel[1], accel[2]
            );
            next_motion = Instant::now() + Duration::from_secs(1);
        }
    }
}

/// Everything but motion, in words. Sticks only show when pushed past a quarter.
fn describe(r: &[u8]) -> String {
    let mut parts = Vec::new();
    let dpad = ["up", "up-right", "right", "down-right", "down", "down-left", "left", "up-left"];
    if let Some(d) = dpad.get((r[5] & 0x0F) as usize) {
        parts.push(format!("dpad {d}"));
    }
    let names = [
        (5, 4, "square"),
        (5, 5, "cross"),
        (5, 6, "circle"),
        (5, 7, "triangle"),
        (6, 0, "L1"),
        (6, 1, "R1"),
        (6, 4, "share"),
        (6, 5, "options"),
        (6, 6, "L3"),
        (6, 7, "R3"),
        (7, 0, "PS"),
        (7, 1, "touchpad click"),
    ];
    for (byte, bit, name) in names {
        if r[byte] >> bit & 1 != 0 {
            parts.push(name.to_string());
        }
    }
    for (at, name) in [(8, "L2"), (9, "R2")] {
        if r[at] > 0 {
            parts.push(format!("{name} {}", r[at]));
        }
    }
    for (at, name) in [(1, "left stick"), (3, "right stick")] {
        let (x, y) = (r[at] as i32 - 128, 128 - r[at + 1] as i32);
        if x.abs() > 32 || y.abs() > 32 {
            parts.push(format!("{name} ({x:+}, {y:+})"));
        }
    }
    let finger = |at: usize| {
        let x = r[at + 1] as u16 | (r[at + 2] as u16 & 0x0F) << 8;
        let y = (r[at + 2] as u16) >> 4 | (r[at + 3] as u16) << 4;
        format!("touch id {} at ({x}, {y})", r[at] & 0x7F)
    };
    for at in [35, 39] {
        if r[at] & 0x80 == 0 {
            parts.push(finger(at));
        }
    }
    let state = if parts.is_empty() { "nothing pressed".to_string() } else { parts.join(", ") };
    format!("{state}   [battery byte {:#04x}]", r[30])
}
