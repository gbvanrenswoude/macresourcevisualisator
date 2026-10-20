//! Where Tessera keeps its files.

use std::path::PathBuf;

pub const APP_NAME: &str = "Tessera";

pub fn home_dir() -> PathBuf {
    if let Some(h) = std::env::var_os("HOME").filter(|h| !h.is_empty()) {
        return PathBuf::from(h);
    }
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if !pw.is_null() && !(*pw).pw_dir.is_null() {
            let dir = std::ffi::CStr::from_ptr((*pw).pw_dir);
            return PathBuf::from(std::ffi::OsStr::from_encoded_bytes_unchecked(
                dir.to_bytes(),
            ));
        }
    }
    PathBuf::from("/")
}

/// `~/Library/Application Support/Tessera` (or `$TESSERA_DATA_DIR`).
pub fn support_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("TESSERA_DATA_DIR") {
        return PathBuf::from(dir);
    }
    home_dir()
        .join("Library/Application Support")
        .join(APP_NAME)
}

pub fn index_file() -> PathBuf {
    support_dir().join("disk-index.bin")
}

pub fn settings_file() -> PathBuf {
    support_dir().join("settings.json")
}
