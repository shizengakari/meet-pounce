use serde::{Deserialize, Serialize};
use std::ffi::OsStr;
use std::fs;
use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;
use std::ptr::{null, null_mut};

#[link(name = "advapi32")]
extern "system" {
    fn RegCreateKeyExW(
        hKey: isize,
        lpSubKey: *const u16,
        Reserved: u32,
        lpClass: *const u16,
        dwOptions: u32,
        samDesired: u32,
        lpSecurityAttributes: usize,
        phkResult: *mut isize,
        lpdwDisposition: *mut u32,
    ) -> i32;
    fn RegOpenKeyExW(
        hKey: isize,
        lpSubKey: *const u16,
        ulOptions: u32,
        samDesired: u32,
        phkResult: *mut isize,
    ) -> i32;
    fn RegSetValueExW(
        hKey: isize,
        lpValueName: *const u16,
        Reserved: u32,
        dwType: u32,
        lpData: *const u8,
        cbData: u32,
    ) -> i32;
    fn RegQueryValueExW(
        hKey: isize,
        lpValueName: *const u16,
        lpReserved: *const u32,
        lpType: *mut u32,
        lpData: *mut u8,
        lpcbData: *mut u32,
    ) -> i32;
    fn RegDeleteValueW(hKey: isize, lpValueName: *const u16) -> i32;
    fn RegCloseKey(hKey: isize) -> i32;
}

const HKEY_CURRENT_USER: isize = 0x8000_0001u32 as i32 as isize;
const KEY_READ: u32 = 0x20019;
const KEY_WRITE: u32 = 0x20006;
const REG_SZ: u32 = 1;
const REG_DWORD: u32 = 4;

const RUN_SUBKEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const APP_REG_NAME: &str = "MeetPounce";

/// 文字列を null 終端 UTF-16 ベクタに変換する
pub fn to_wide(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
}

/// レジストリに文字列値を書き込む
pub fn reg_set_sz(subkey: &str, name: &str, value: &str) -> bool {
    let subkey_w = to_wide(subkey);
    let name_w = to_wide(name);
    let val_w = to_wide(value);
    unsafe {
        let mut hkey = 0;
        if RegCreateKeyExW(HKEY_CURRENT_USER, subkey_w.as_ptr(), 0, null(), 0, KEY_WRITE, 0, &mut hkey, null_mut()) != 0 {
            return false;
        }
        let res = RegSetValueExW(
            hkey,
            name_w.as_ptr(),
            0,
            REG_SZ,
            val_w.as_ptr() as *const u8,
            (val_w.len() * 2) as u32,
        );
        RegCloseKey(hkey);
        res == 0
    }
}

/// レジストリに 32bit 整数値を書き込む
pub fn reg_set_dword(subkey: &str, name: &str, value: u32) -> bool {
    let subkey_w = to_wide(subkey);
    let name_w = to_wide(name);
    unsafe {
        let mut hkey = 0;
        if RegCreateKeyExW(HKEY_CURRENT_USER, subkey_w.as_ptr(), 0, null(), 0, KEY_WRITE, 0, &mut hkey, null_mut()) != 0 {
            return false;
        }
        let bytes = value.to_le_bytes();
        let res = RegSetValueExW(hkey, name_w.as_ptr(), 0, REG_DWORD, bytes.as_ptr(), 4);
        RegCloseKey(hkey);
        res == 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    pub email: String,
    pub refresh_token: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppConfig {
    pub client_id: String,
    pub client_secret: String,
    pub accounts: Vec<Account>,

    // オンライン会議の通知設定
    #[serde(default = "default_true")]
    pub meeting_enabled: bool,
    #[serde(default = "default_minutes")]
    pub notify_minutes_before: u32,
    #[serde(default = "default_sound_type")]
    pub sound_type: String,
    #[serde(default = "default_sound_volume")]
    pub sound_volume: u32,

    // 通常の予定の通知設定
    #[serde(default = "default_true")]
    pub event_enabled: bool,
    #[serde(default = "default_minutes")]
    pub event_notify_minutes_before: u32,
    #[serde(default = "default_sound_type")]
    pub event_sound_type: String,
    #[serde(default = "default_sound_volume")]
    pub event_sound_volume: u32,
}

fn default_true() -> bool { true }
fn default_minutes() -> u32 { 1 }
fn default_sound_type() -> String { "calendar".to_string() }
fn default_sound_volume() -> u32 { 200 }

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            client_id: String::new(),
            client_secret: String::new(),
            accounts: Vec::new(),
            meeting_enabled: true,
            notify_minutes_before: 1,
            sound_type: default_sound_type(),
            sound_volume: 200,
            event_enabled: true,
            event_notify_minutes_before: 1,
            event_sound_type: default_sound_type(),
            event_sound_volume: 200,
        }
    }
}

impl AppConfig {
    pub fn file_path() -> PathBuf {
        if let Some(appdata) = std::env::var_os("APPDATA") {
            let dir = PathBuf::from(appdata).join(r"meetpounce\meet-pounce\config");
            let _ = fs::create_dir_all(&dir);
            dir.join("config.json")
        } else {
            PathBuf::from("config.json")
        }
    }

    pub fn load() -> Self {
        fs::read_to_string(Self::file_path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), std::io::Error> {
        let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        fs::write(Self::file_path(), json)
    }

    /// 予定種別に応じた通知設定を返す
    pub fn rule_for(&self, is_online: bool) -> (bool, i64, &str, u32) {
        if is_online {
            (self.meeting_enabled, self.notify_minutes_before as i64, &self.sound_type, self.sound_volume)
        } else {
            (self.event_enabled, self.event_notify_minutes_before as i64, &self.event_sound_type, self.event_sound_volume)
        }
    }

    pub fn add_or_update_account(&mut self, email: String, refresh_token: String) {
        if let Some(acc) = self.accounts.iter_mut().find(|a| a.email == email) {
            acc.refresh_token = refresh_token;
        } else {
            self.accounts.push(Account { email, refresh_token });
        }
        let _ = self.save();
    }

    pub fn remove_account(&mut self, email: &str) {
        self.accounts.retain(|a| a.email != email);
        let _ = self.save();
    }
}

/// スタートアップ自動起動を設定する
pub fn set_startup(enable: bool) -> Result<(), std::io::Error> {
    if enable {
        let exe = std::env::current_exe()?;
        let val = format!("\"{}\"", exe.to_string_lossy());
        if !reg_set_sz(RUN_SUBKEY, APP_REG_NAME, &val) {
            return Err(std::io::Error::other("レジストリ登録に失敗しました"));
        }
    } else {
        let subkey_w = to_wide(RUN_SUBKEY);
        let name_w = to_wide(APP_REG_NAME);
        unsafe {
            let mut hkey = 0;
            if RegOpenKeyExW(HKEY_CURRENT_USER, subkey_w.as_ptr(), 0, KEY_WRITE, &mut hkey) == 0 {
                RegDeleteValueW(hkey, name_w.as_ptr());
                RegCloseKey(hkey);
            }
        }
    }
    Ok(())
}

/// スタートアップ自動起動が有効か判定する
pub fn is_startup_enabled() -> bool {
    let subkey_w = to_wide(RUN_SUBKEY);
    let name_w = to_wide(APP_REG_NAME);
    unsafe {
        let mut hkey = 0;
        if RegOpenKeyExW(HKEY_CURRENT_USER, subkey_w.as_ptr(), 0, KEY_READ, &mut hkey) != 0 {
            return false;
        }
        let exists = RegQueryValueExW(hkey, name_w.as_ptr(), null(), null_mut(), null_mut(), null_mut()) == 0;
        RegCloseKey(hkey);
        exists
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_app_config_default_and_rule() {
        let cfg = AppConfig::default();
        assert_eq!(cfg.client_id, "");
        assert!(cfg.meeting_enabled);
        assert!(cfg.event_enabled);
        assert_eq!(cfg.rule_for(true), (true, 1, "calendar", 200));
        assert_eq!(cfg.rule_for(false), (true, 1, "calendar", 200));
    }

    #[test]
    fn test_config_json_roundtrip() {
        let original = AppConfig {
            client_id: "id".to_string(),
            client_secret: "sec".to_string(),
            meeting_enabled: true,
            notify_minutes_before: 3,
            sound_type: "alarm".to_string(),
            sound_volume: 300,
            event_enabled: false,
            event_notify_minutes_before: 5,
            event_sound_type: "chimes".to_string(),
            event_sound_volume: 150,
            accounts: vec![Account {
                email: "test@domain.com".to_string(),
                refresh_token: "token".to_string(),
            }],
        };

        let json = serde_json::to_string(&original).unwrap();
        let restored: AppConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(restored.rule_for(true), (true, 3, "alarm", 300));
        assert_eq!(restored.rule_for(false), (false, 5, "chimes", 150));
        assert_eq!(restored.accounts.len(), 1);
    }
}
