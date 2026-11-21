use std::fs;
use std::path::Path;

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW,
    RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE,
    REG_SZ,
};

const RUN_SUBKEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const VALUE_NAME: PCWSTR = w!("ClipSync");
const INIT_MARKER: &str = "autostart_init";

pub fn ensure_default_enabled(data_dir: &Path) {
    let marker = data_dir.join(INIT_MARKER);
    if marker.exists() {
        return;
    }
    let _ = fs::create_dir_all(data_dir);
    let _ = set_enabled(true);
    let _ = fs::write(&marker, b"1");
}

pub fn is_enabled() -> bool {
    unsafe {
        let mut hkey = HKEY::default();
        if RegOpenKeyExW(HKEY_CURRENT_USER, RUN_SUBKEY, 0, KEY_READ, &mut hkey) != ERROR_SUCCESS {
            return false;
        }
        let exists =
            RegQueryValueExW(hkey, VALUE_NAME, None, None, None, None) == ERROR_SUCCESS;
        let _ = RegCloseKey(hkey);
        exists
    }
}

pub fn set_enabled(enable: bool) -> bool {
    unsafe {
        let mut hkey = HKEY::default();
        if RegCreateKeyExW(
            HKEY_CURRENT_USER,
            RUN_SUBKEY,
            0,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut hkey,
            None,
        ) != ERROR_SUCCESS
        {
            return false;
        }

        let ok = if enable {
            if let Ok(exe_path) = std::env::current_exe() {
                let quoted = format!("\"{}\"", exe_path.to_string_lossy());
                let mut wide: Vec<u16> = quoted.encode_utf16().collect();
                wide.push(0);
                let byte_slice =
                    std::slice::from_raw_parts(wide.as_ptr() as *const u8, wide.len() * 2);
                RegSetValueExW(hkey, VALUE_NAME, 0, REG_SZ, Some(byte_slice)) == ERROR_SUCCESS
            } else {
                false
            }
        } else {
            let res = RegDeleteValueW(hkey, VALUE_NAME);
            res == ERROR_SUCCESS || res == ERROR_FILE_NOT_FOUND
        };

        let _ = RegCloseKey(hkey);
        ok
    }
}
