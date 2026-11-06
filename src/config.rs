use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Account {
    pub email: String,
    pub refresh_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub client_id: String,
    pub client_secret: String,
    pub accounts: Vec<Account>,
    pub notify_minutes_before: u32,
    pub sound_type: String, // "calendar", "chimes", "ring", "alarm", "generic", "silent"
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            client_id: String::new(),
            client_secret: String::new(),
            accounts: Vec::new(),
            notify_minutes_before: 1,
            sound_type: "calendar".to_string(),
        }
    }
}

impl AppConfig {
    pub fn file_path() -> PathBuf {
        if let Some(dirs) = ProjectDirs::from("com", "meetpounce", "meet-pounce") {
            let path = dirs.config_dir();
            let _ = fs::create_dir_all(path);
            path.join("config.json")
        } else {
            PathBuf::from("config.json")
        }
    }

    pub fn load() -> Self {
        let path = Self::file_path();
        if path.exists() {
            if let Ok(content) = fs::read_to_string(&path) {
                if let Ok(cfg) = serde_json::from_str(&content) {
                    return cfg;
                }
            }
        }
        Self::default()
    }

    pub fn save(&self) -> Result<(), std::io::Error> {
        let path = Self::file_path();
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        fs::write(path, json)
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

/// Windowsのスタートアップ自動起動を登録/解除する
pub fn set_startup(enable: bool) -> Result<(), std::io::Error> {
    let exe_path = std::env::current_exe()?;
    let path_str = exe_path.to_string_lossy();
    if enable {
        let status = Command::new("reg")
            .args([
                "add",
                "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run",
                "/v",
                "MeetPounce",
                "/t",
                "REG_SZ",
                "/d",
                &format!("\"{}\"", path_str),
                "/f",
            ])
            .status()?;
        if !status.success() {
            return Err(std::io::Error::new(std::io::ErrorKind::Other, "レジストリ登録に失敗しました"));
        }
    } else {
        let _ = Command::new("reg")
            .args([
                "delete",
                "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run",
                "/v",
                "MeetPounce",
                "/f",
            ])
            .status();
    }
    Ok(())
}

/// Windowsのスタートアップ自動起動が有効かどうかを判定する
pub fn is_startup_enabled() -> bool {
    Command::new("reg")
        .args([
            "query",
            "HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Run",
            "/v",
            "MeetPounce",
        ])
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_app_config_default() {
        let cfg = AppConfig::default();
        assert_eq!(cfg.client_id, "");
        assert_eq!(cfg.client_secret, "");
        assert_eq!(cfg.notify_minutes_before, 1);
        assert_eq!(cfg.sound_type, "calendar");
        assert!(cfg.accounts.is_empty());
    }

    #[test]
    fn test_add_or_update_account() {
        let mut cfg = AppConfig::default();

        // 新規追加
        cfg.accounts.push(Account {
            email: "alice@example.com".to_string(),
            refresh_token: "token1".to_string(),
        });
        assert_eq!(cfg.accounts.len(), 1);
        assert_eq!(cfg.accounts[0].refresh_token, "token1");

        // 既存更新
        if let Some(acc) = cfg.accounts.iter_mut().find(|a| a.email == "alice@example.com") {
            acc.refresh_token = "token2".to_string();
        }
        assert_eq!(cfg.accounts.len(), 1);
        assert_eq!(cfg.accounts[0].refresh_token, "token2");

        // 別アカウント追加
        cfg.accounts.push(Account {
            email: "bob@example.com".to_string(),
            refresh_token: "token3".to_string(),
        });
        assert_eq!(cfg.accounts.len(), 2);
    }

    #[test]
    fn test_remove_account() {
        let mut cfg = AppConfig::default();
        cfg.accounts.push(Account {
            email: "user1@example.com".to_string(),
            refresh_token: "token1".to_string(),
        });
        cfg.accounts.push(Account {
            email: "user2@example.com".to_string(),
            refresh_token: "token2".to_string(),
        });

        // 削除
        cfg.accounts.retain(|a| a.email != "user1@example.com");
        assert_eq!(cfg.accounts.len(), 1);
        assert_eq!(cfg.accounts[0].email, "user2@example.com");

        // 存在しないアカウント削除で変化なし
        cfg.accounts.retain(|a| a.email != "nonexistent@example.com");
        assert_eq!(cfg.accounts.len(), 1);
    }

    #[test]
    fn test_config_json_roundtrip() {
        let mut original = AppConfig::default();
        original.client_id = "test-client-id".to_string();
        original.client_secret = "test-client-secret".to_string();
        original.notify_minutes_before = 5;
        original.sound_type = "alarm".to_string();
        original.accounts.push(Account {
            email: "test@domain.com".to_string(),
            refresh_token: "secret-token".to_string(),
        });

        let json = serde_json::to_string(&original).expect("serialize failed");
        let restored: AppConfig = serde_json::from_str(&json).expect("deserialize failed");

        assert_eq!(restored.client_id, original.client_id);
        assert_eq!(restored.client_secret, original.client_secret);
        assert_eq!(restored.notify_minutes_before, original.notify_minutes_before);
        assert_eq!(restored.sound_type, original.sound_type);
        assert_eq!(restored.accounts.len(), 1);
        assert_eq!(restored.accounts[0].email, "test@domain.com");
        assert_eq!(restored.accounts[0].refresh_token, "secret-token");
    }
}
