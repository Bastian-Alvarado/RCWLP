//! "Start with Windows": a value under HKCU\Software\Microsoft\Windows\CurrentVersion\Run, the
//! per-user list Windows runs at sign-in. No admin rights needed.

use std::io;
use std::path::Path;
use std::ptr;

use winapi::shared::minwindef::{DWORD, HKEY};
use winapi::shared::winerror::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use winapi::um::winnt::{KEY_READ, KEY_WRITE, REG_SZ};
use winapi::um::winreg::{
    HKEY_CURRENT_USER, RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW,
    RegSetValueExW,
};

/// A named value in a key under HKEY_CURRENT_USER.
pub struct RunEntry {
    pub key: &'static str,
    pub value: &'static str,
}

/// The real entry: RCWLP in the user's Run key.
pub const RCWLP: RunEntry = RunEntry { key: r"Software\Microsoft\Windows\CurrentVersion\Run", value: "RCWLP" };

/// The command line that starts `exe`, quoted for paths with spaces.
pub fn command_for(exe: &Path) -> String {
    format!("\"{}\"", exe.display())
}

impl RunEntry {
    /// The command stored, or None if there is none.
    pub fn get(&self) -> Option<String> {
        let key = Key::open(self.key, KEY_READ).ok()?;
        let name = wide(self.value);
        let mut size: DWORD = 0;
        // Safety: a null data pointer asks for the size only.
        let status = unsafe { RegQueryValueExW(key.0, name.as_ptr(), ptr::null_mut(), ptr::null_mut(), ptr::null_mut(), &mut size) };
        if status != ERROR_SUCCESS as i32 || size == 0 {
            return None;
        }
        let mut data = vec![0u16; (size as usize).div_ceil(2)];
        let mut kind: DWORD = 0;
        // Safety: `data` holds `size` bytes.
        let status = unsafe {
            RegQueryValueExW(key.0, name.as_ptr(), ptr::null_mut(), &mut kind, data.as_mut_ptr().cast(), &mut size)
        };
        if status != ERROR_SUCCESS as i32 || kind != REG_SZ {
            return None;
        }
        let end = data.iter().position(|&c| c == 0).unwrap_or(data.len());
        Some(String::from_utf16_lossy(&data[..end]))
    }

    pub fn set(&self, command: &str) -> io::Result<()> {
        let key = Key::create(self.key)?;
        let name = wide(self.value);
        let data = wide(command);
        let bytes = (data.len() * 2) as DWORD;
        // Safety: `data` is a NUL-terminated UTF-16 string of `bytes` bytes.
        let status = unsafe { RegSetValueExW(key.0, name.as_ptr(), 0, REG_SZ, data.as_ptr().cast(), bytes) };
        check(status)
    }

    /// Removes the value. Not having one is fine.
    pub fn remove(&self) -> io::Result<()> {
        let key = match Key::open(self.key, KEY_WRITE) {
            Ok(k) => k,
            Err(e) if e.raw_os_error() == Some(ERROR_FILE_NOT_FOUND as i32) => return Ok(()),
            Err(e) => return Err(e),
        };
        let name = wide(self.value);
        // Safety: plain call with a NUL-terminated name.
        let status = unsafe { RegDeleteValueW(key.0, name.as_ptr()) };
        if status == ERROR_FILE_NOT_FOUND as i32 { Ok(()) } else { check(status) }
    }
}

struct Key(HKEY);

impl Key {
    fn open(path: &str, access: DWORD) -> io::Result<Key> {
        let mut key: HKEY = ptr::null_mut();
        let path = wide(path);
        // Safety: `key` receives the handle, closed by Drop.
        check(unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, path.as_ptr(), 0, access, &mut key) })?;
        Ok(Key(key))
    }

    fn create(path: &str) -> io::Result<Key> {
        let mut key: HKEY = ptr::null_mut();
        let path = wide(path);
        // Safety: as above; the key is created if missing.
        check(unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                path.as_ptr(),
                0,
                ptr::null_mut(),
                0,
                KEY_READ | KEY_WRITE,
                ptr::null_mut(),
                &mut key,
                ptr::null_mut(),
            )
        })?;
        Ok(Key(key))
    }
}

impl Drop for Key {
    fn drop(&mut self) {
        // Safety: the handle came from RegOpenKeyExW or RegCreateKeyExW.
        unsafe { RegCloseKey(self.0) };
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn check(status: i32) -> io::Result<()> {
    if status == ERROR_SUCCESS as i32 { Ok(()) } else { Err(io::Error::from_raw_os_error(status)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scratch key, so the test never touches the real Run key.
    const SCRATCH: RunEntry = RunEntry { key: r"Software\RCWLP-test-autostart", value: "RCWLP" };

    use winapi::um::winreg::RegDeleteKeyW;

    #[test]
    fn set_get_remove() {
        let _ = SCRATCH.remove();
        assert_eq!(SCRATCH.get(), None);
        let command = command_for(Path::new(r"C:\Program Files\RCWLP\rcwlp-tray.exe"));
        assert_eq!(command, r#""C:\Program Files\RCWLP\rcwlp-tray.exe""#);
        SCRATCH.set(&command).unwrap();
        assert_eq!(SCRATCH.get().as_deref(), Some(command.as_str()));
        SCRATCH.remove().unwrap();
        assert_eq!(SCRATCH.get(), None);
        SCRATCH.remove().unwrap();
        let key = wide(SCRATCH.key);
        // Safety: deletes the empty scratch key.
        unsafe { RegDeleteKeyW(HKEY_CURRENT_USER, key.as_ptr()) };
    }
}
