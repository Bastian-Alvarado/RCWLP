//! Shows what a game would read from the virtual DualShock 4: touchpad fingers, touchpad
//! click, PS button and battery. Handy to check the phone's touchpad end to end.
//!
//!   cargo run --example ds4_watch -- [seconds] [--feedback R G B LARGE SMALL]
//!
//! `--feedback` first sends what a game would: a lightbar colour and rumble (0-255 each).
//! The phone should take on the colour and vibrate.

use std::time::{Duration, Instant};

const SONY: u16 = 0x054C;
const DS4: u16 = 0x05C4;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let seconds: u64 = args.first().and_then(|s| s.parse().ok()).unwrap_or(20);
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
    let mut buf = [0u8; 64];
    let mut last = String::new();
    while Instant::now() < deadline {
        let Ok(n) = device.read_timeout(&mut buf, 100) else { break };
        // USB input report 0x01; offsets below are the report's, report id included.
        if n < 43 || buf[0] != 0x01 {
            continue;
        }
        let finger = |at: usize| {
            if buf[at] & 0x80 != 0 {
                "up".to_string()
            } else {
                let x = buf[at + 1] as u16 | (buf[at + 2] as u16 & 0x0F) << 8;
                let y = (buf[at + 2] as u16) >> 4 | (buf[at + 3] as u16) << 4;
                format!("id {} at ({x}, {y})", buf[at] & 0x7F)
            }
        };
        let now = format!(
            "finger 0 {}, finger 1 {}, click {}, PS {}, battery byte {:#04x}",
            finger(35),
            finger(39),
            buf[7] >> 1 & 1,
            buf[7] & 1,
            buf[30]
        );
        if now != last {
            println!("{now}");
            last = now;
        }
    }
}
