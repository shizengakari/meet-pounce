#![windows_subsystem = "windows"]

mod auth;
mod calendar;
mod config;
mod notification;
mod settings_server;
mod tray;

use calendar::MeetingEvent;
use chrono::{Duration, Local};
use config::AppConfig;
use std::collections::HashSet;
use std::os::windows::ffi::OsStrExt;
use std::sync::Arc;
use tokio::sync::{mpsc, watch, RwLock};
use tray::TrayCommand;

#[repr(C)]
struct SystemPowerStatus {
    ac_line_status: u8,
    battery_flag: u8,
    battery_life_percent: u8,
    system_status_flag: u8,
    battery_life_time: u32,
    battery_full_life_time: u32,
}

#[link(name = "kernel32")]
extern "system" {
    fn GetSystemPowerStatus(lp: *mut SystemPowerStatus) -> i32;
    fn CreateMutexW(lpMutexAttributes: usize, bInitialOwner: i32, lpName: *const u16) -> isize;
    fn GetLastError() -> u32;
}

const ERROR_ALREADY_EXISTS: u32 = 183;

/// 多重起動を防止するための名前付きミューテックス (1プロセスのみ許可)
fn acquire_single_instance_lock() -> bool {
    let name: Vec<u16> = std::ffi::OsStr::new("Local\\MeetPounce_SingleInstance_Mutex")
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        let handle = CreateMutexW(0, 1, name.as_ptr());
        !(handle != 0 && GetLastError() == ERROR_ALREADY_EXISTS)
    }
}

/// Windowsの電源状態を判定 (バッテリー駆動なら同期頻度を半減)
fn is_on_battery() -> bool {
    let mut status = SystemPowerStatus {
        ac_line_status: 255,
        battery_flag: 0,
        battery_life_percent: 0,
        system_status_flag: 0,
        battery_life_time: 0,
        battery_full_life_time: 0,
    };
    unsafe { GetSystemPowerStatus(&mut status) != 0 && status.ac_line_status == 0 }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let args: Vec<String> = std::env::args().collect();

    // 1. テスト通知オプション (--test)
    if args.iter().any(|a| a == "--test") {
        let config = AppConfig::load();
        notification::show_meeting_toast(
            "開発定例ミーティング",
            "Meet",
            Some("https://meet.google.com/test"),
            1,
            &config.sound_type,
        )?;
        return Ok(());
    }

    // 2. 設定画面オプション (--settings / --config)
    if args.iter().any(|a| a == "--settings" || a == "--config") {
        settings_server::run_settings_server().await?;
        return Ok(());
    }

    // 3. 多重起動防止 (すでに常駐中なら二重起動しない)
    if !acquire_single_instance_lock() {
        return Ok(());
    }

    // 4. 【最優先】タスクトレイアイコンを直ちに起動・表示する！
    let (tray_tx, mut tray_rx) = mpsc::unbounded_channel::<TrayCommand>();
    tray::init_tray(tray_tx)?;

    let config = Arc::new(RwLock::new(AppConfig::load()));
    let meetings_state = Arc::new(RwLock::new(Vec::<MeetingEvent>::new()));
    let (reschedule_tx, mut reschedule_rx) = watch::channel(());
    let (manual_sync_tx, mut manual_sync_rx) = mpsc::unbounded_channel::<()>();

    // 初回起動で未設定なら、バックグラウンドで設定画面を開く (トレイ常駐を維持したまま)
    {
        let c = config.read().await;
        if c.client_id.is_empty() || c.accounts.is_empty() {
            tokio::spawn(async {
                let _ = settings_server::run_settings_server().await;
            });
        }
    }

    // 5. トレイメニューのコマンド処理ループ
    let cfg_tray = Arc::clone(&config);
    let manual_sync_tx_clone = manual_sync_tx.clone();
    tokio::spawn(async move {
        while let Some(cmd) = tray_rx.recv().await {
            match cmd {
                TrayCommand::OpenSettings => {
                    tokio::spawn(async {
                        let _ = settings_server::run_settings_server().await;
                    });
                }
                TrayCommand::SyncNow => {
                    let _ = manual_sync_tx_clone.send(());
                }
                TrayCommand::TestToast => {
                    let sound = cfg_tray.read().await.sound_type.clone();
                    let _ = notification::show_meeting_toast(
                        "通知テストミーティング",
                        "Meet",
                        Some("https://meet.google.com/test"),
                        1,
                        &sound,
                    );
                }
                TrayCommand::Quit => {
                    std::process::exit(0);
                }
            }
        }
    });

    // 6. カレンダー同期タスク (省電力ポーリング ＆ 手動割り込み)
    let cfg_sync = Arc::clone(&config);
    let mtg_sync = Arc::clone(&meetings_state);
    let tx_sync = reschedule_tx.clone();

    tokio::spawn(async move {
        let client = reqwest::Client::new();
        loop {
            // 設定ファイルを再読み込み (設定画面で変更された場合を反映)
            {
                let fresh_cfg = AppConfig::load();
                let mut c = cfg_sync.write().await;
                *c = fresh_cfg;
            }

            let (cid, csec, accounts) = {
                let c = cfg_sync.read().await;
                (c.client_id.clone(), c.client_secret.clone(), c.accounts.clone())
            };

            if !cid.is_empty() && !accounts.is_empty() {
                let mut all_events = Vec::new();
                for acc in accounts {
                    if let Ok(token) = auth::refresh_access_token(&cid, &csec, &acc.refresh_token).await {
                        if let Ok(events) = calendar::fetch_today_events(&client, &token).await {
                            all_events.extend(events);
                        }
                    }
                }

                all_events.sort_by_key(|e| e.start_time);
                {
                    let mut state = mtg_sync.write().await;
                    *state = all_events;
                }
                let _ = tx_sync.send(());
            }

            // バッテリー時は30分、AC電源時は10分待機 (手動同期ボタンで即時解除)
            let sleep_mins = if is_on_battery() { 30 } else { 10 };
            tokio::select! {
                _ = tokio::time::sleep(tokio::time::Duration::from_secs(sleep_mins * 60)) => {}
                Some(_) = manual_sync_rx.recv() => {
                    // 手動同期
                }
            }
        }
    });

    // 7. 超低消費電力タイマースケジューラー (イベントドリブン)
    let cfg_sched = Arc::clone(&config);
    let mtg_sched = Arc::clone(&meetings_state);
    let mut notified_ids = HashSet::new();

    loop {
        let now = Local::now();
        let (notify_mins, sound_type) = {
            let c = cfg_sched.read().await;
            (c.notify_minutes_before as i64, c.sound_type.clone())
        };

        // 次の通知対象ミーティングを探索
        let next_meeting = {
            let state = mtg_sched.read().await;
            state
                .iter()
                .filter(|m| !notified_ids.contains(&m.id))
                .find(|m| m.start_time > now - Duration::minutes(2))
                .cloned()
        };

        if let Some(mtg) = next_meeting {
            let trigger_time = mtg.start_time - Duration::minutes(notify_mins);
            let wait_secs = (trigger_time - now).num_seconds().max(0) as u64;

            // トレイのツールチップを更新
            tray::update_tray_tooltip(&format!(
                "Meet Pounce - 次: {} ({})",
                mtg.start_time.format("%H:%M"),
                mtg.title
            ));

            // OSタイマースリープ (予定変更があったら即時中断して再スケジュール)
            tokio::select! {
                _ = tokio::time::sleep(tokio::time::Duration::from_secs(wait_secs)) => {
                    notified_ids.insert(mtg.id.clone());
                    let _ = notification::show_meeting_toast(
                        &mtg.title,
                        &mtg.service_type,
                        mtg.meeting_url.as_deref(),
                        notify_mins,
                        &sound_type,
                    );
                }
                _ = reschedule_rx.changed() => {
                    // カレンダー更新で再計算
                }
            }
        } else {
            let c = cfg_sched.read().await;
            if c.client_id.is_empty() || c.accounts.is_empty() {
                tray::update_tray_tooltip("Meet Pounce - 初期設定が必要です (右クリックで設定)");
            } else {
                tray::update_tray_tooltip("Meet Pounce - 本日の残り予定なし");
            }
            let _ = reschedule_rx.changed().await;
        }
    }
}
