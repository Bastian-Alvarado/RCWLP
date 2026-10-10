//! Local wall-clock time for log lines, without pulling in a date library.

use winapi::um::minwinbase::SYSTEMTIME;
use winapi::um::sysinfoapi::GetLocalTime;

/// "[HH:MM:SS]" in local time.
pub fn stamp() -> String {
    // Safety: GetLocalTime only writes the struct it's given.
    let t = unsafe {
        let mut t: SYSTEMTIME = std::mem::zeroed();
        GetLocalTime(&mut t);
        t
    };
    format!("[{:02}:{:02}:{:02}]", t.wHour, t.wMinute, t.wSecond)
}
