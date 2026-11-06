#![windows_subsystem = "windows"]

mod auth;
mod calendar;
mod config;
mod notification;
mod settings_server;
mod tray;

use calendar::{FetchResult, MeetingEvent};
use chrono::{DateTime, Duration, Local};
use config::{to_wide, Account, AppConfig};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
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
    fn GetCurrentProcess() -> isize;
    fn SetProcessWorkingSetSize(hProcess: isize, min: usize, max: usize) -> i32;
}

const ERROR_ALREADY_EXISTS: u32 = 183;

/// 多重起動防止ロックを取得する
fn acquire_single_instance_lock() -> bool {
    let name = to_wide("Local\\MeetPounce_SingleInstance_Mutex");
    unsafe {
        let handle = CreateMutexW(0, 1, name.as_ptr());
        !(handle != 0 && GetLastError() == ERROR_ALREADY_EXISTS)
    }
}

/// バッテリー駆動中か判定する
fn is_on_battery() -> bool {
    let mut status: SystemPowerStatus = unsafe { std::mem::zeroed() };
    status.ac_line_status = 255;
    unsafe { GetSystemPowerStatus(&mut status) != 0 && status.ac_line_status == 0 }
}

/// 未使用の物理メモリページをOSへ返却する
pub fn trim_working_set() {
    unsafe {
        SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX);
    }
}

/// 設定画面を開き、終了後に同期とメモリ解放を行う
fn spawn_settings(sync_tx: mpsc::UnboundedSender<()>) {
    tokio::spawn(async move {
        let _ = settings_server::run_settings_server().await;
        let _ = sync_tx.send(());
        trim_working_set();
    });
}

#[derive(Default)]
struct AccountCache {
    token: Option<(String, Instant)>,
    etag: Option<String>,
    events: Vec<MeetingEvent>,
}

/// 1アカウント分の予定を差分同期する
async fn sync_single_account(
    client: &reqwest::Client,
    cfg: &AppConfig,
    acc: &Account,
    cache: &mut AccountCache,
    cutoff: DateTime<Local>,
) {
    let valid_tok = cache
        .token
        .as_ref()
        .filter(|(_, ts)| ts.elapsed().as_secs() < 3000)
        .map(|(t, _)| t.clone());

    let token = match valid_tok {
        Some(t) => t,
        None => {
            let Ok(t) = auth::refresh_access_token(client, &cfg.client_id, &cfg.client_secret, &acc.refresh_token).await else {
                cache.events.retain(|e| e.start_time > cutoff);
                return;
            };
            cache.token = Some((t.clone(), Instant::now()));
            t
        }
    };

    let res = match calendar::fetch_today_events(client, &token, cache.etag.as_deref()).await {
        Ok(r) => Ok(r),
        Err(_) => {
            if let Ok(new_tok) = auth::refresh_access_token(client, &cfg.client_id, &cfg.client_secret, &acc.refresh_token).await {
                cache.token = Some((new_tok.clone(), Instant::now()));
                calendar::fetch_today_events(client, &new_tok, None).await
            } else {
                Err("refresh failed".into())
            }
        }
    };

    match res {
        Ok(FetchResult::Updated(events, new_etag)) => {
            cache.etag = new_etag;
            cache.events = events;
        }
        Ok(FetchResult::Unchanged) | Err(_) => {
            cache.events.retain(|e| e.start_time > cutoff);
        }
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let args: Vec<String> = std::env::args().collect();

    if args.iter().any(|a| a == "--test") {
        let cfg = AppConfig::load();
        notification::show_meeting_toast(
            "開発定例ミーティング",
            "Meet",
            Some("https://meet.google.com"),
            1,
            &cfg.sound_type,
            cfg.sound_volume,
        )?;
        std::thread::sleep(std::time::Duration::from_secs(2));
        return Ok(());
    }

    if args.iter().any(|a| a == "--settings" || a == "--config") {
        settings_server::run_settings_server().await?;
        return Ok(());
    }

    if !acquire_single_instance_lock() {
        return Ok(());
    }

    let (tray_tx, mut tray_rx) = mpsc::unbounded_channel::<TrayCommand>();
    tray::init_tray(tray_tx)?;

    let config = Arc::new(RwLock::new(AppConfig::load()));
    let meetings_state = Arc::new(RwLock::new(Vec::<MeetingEvent>::new()));
    let (reschedule_tx, mut reschedule_rx) = watch::channel(());
    let (manual_sync_tx, mut manual_sync_rx) = mpsc::unbounded_channel::<()>();

    // 初回未設定時はバックグラウンドで設定画面を開く
    {
        let c = config.read().await;
        if c.client_id.is_empty() || c.accounts.is_empty() {
            spawn_settings(manual_sync_tx.clone());
        }
    }

    // トレイメニューのコマンド処理
    let cfg_tray = Arc::clone(&config);
    let sync_tx_tray = manual_sync_tx.clone();
    tokio::spawn(async move {
        while let Some(cmd) = tray_rx.recv().await {
            match cmd {
                TrayCommand::OpenSettings => spawn_settings(sync_tx_tray.clone()),
                TrayCommand::SyncNow => {
                    let _ = sync_tx_tray.send(());
                }
                TrayCommand::TestToast => {
                    let c = cfg_tray.read().await;
                    let _ = notification::show_meeting_toast(
                        "通知テストミーティング",
                        "Meet",
                        Some("https://meet.google.com"),
                        1,
                        &c.sound_type,
                        c.sound_volume,
                    );
                }
                TrayCommand::Quit => std::process::exit(0),
            }
        }
    });

    // カレンダー同期タスク
    let cfg_sync = Arc::clone(&config);
    let mtg_sync = Arc::clone(&meetings_state);
    let tx_sync = reschedule_tx.clone();

    tokio::spawn(async move {
        let client = reqwest::Client::new();
        let mut caches: HashMap<String, AccountCache> = HashMap::new();
        let mut last_date = Local::now().date_naive();

        loop {
            let now = Local::now();
            let today = now.date_naive();
            let cutoff = now - Duration::minutes(2);

            if today != last_date {
                for c in caches.values_mut() {
                    c.etag = None;
                    c.events.clear();
                }
                last_date = today;
            }

            let fresh_cfg = AppConfig::load();
            caches.retain(|email, _| fresh_cfg.accounts.iter().any(|a| &a.email == email));

            let can_sync = !fresh_cfg.client_id.is_empty()
                && !fresh_cfg.accounts.is_empty()
                && (fresh_cfg.meeting_enabled || fresh_cfg.event_enabled);

            let mut all_events = Vec::new();

            if can_sync {
                for acc in &fresh_cfg.accounts {
                    let entry = caches.entry(acc.email.clone()).or_default();
                    sync_single_account(&client, &fresh_cfg, acc, entry, cutoff).await;

                    all_events.extend(
                        entry
                            .events
                            .iter()
                            .filter(|e| if e.meeting_url.is_some() { fresh_cfg.meeting_enabled } else { fresh_cfg.event_enabled })
                            .cloned(),
                    );
                }

                all_events.sort_by_key(|e| e.start_time);
                all_events.dedup_by(|a, b| a.id == b.id || (a.start_time == b.start_time && a.title == b.title));
            }

            // 変更時のみスケジューラーへ通知する
            let cfg_changed = {
                let mut c = cfg_sync.write().await;
                let changed = *c != fresh_cfg;
                if changed {
                    *c = fresh_cfg;
                }
                changed
            };

            let events_changed = {
                let mut state = mtg_sync.write().await;
                let changed = *state != all_events;
                if changed {
                    *state = all_events;
                }
                changed
            };

            if cfg_changed || events_changed {
                let _ = tx_sync.send(());
            }

            trim_working_set();

            let sleep_mins = if is_on_battery() { 30 } else { 10 };
            tokio::select! {
                _ = tokio::time::sleep(tokio::time::Duration::from_secs(sleep_mins * 60)) => {}
                Some(_) = manual_sync_rx.recv() => {}
            }
        }
    });

    // 通知スケジューラー
    let mut notified_ids: HashMap<String, DateTime<Local>> = HashMap::new();

    loop {
        let now = Local::now();
        let cutoff = now - Duration::minutes(2);

        let (next_target, is_configured) = {
            let cfg = config.read().await;
            let mut state = meetings_state.write().await;
            notified_ids.retain(|_, &mut start| start > cutoff);
            notified_ids.shrink_to_fit();
            state.retain(|m| !notified_ids.contains_key(&m.id) && m.start_time > cutoff);
            state.shrink_to_fit();

            let target = state
                .iter()
                .filter_map(|m| {
                    let (enabled, mins, sound, vol) = cfg.rule_for(m.meeting_url.is_some());
                    enabled.then(|| {
                        let trigger = m.start_time - Duration::minutes(mins);
                        (m.clone(), trigger, mins, sound.to_string(), vol)
                    })
                })
                .min_by_key(|(_, trigger, _, _, _)| *trigger);

            (target, !cfg.client_id.is_empty() && !cfg.accounts.is_empty())
        };

        if let Some((mtg, trigger_time, mins, sound, vol)) = next_target {
            let wait_secs = (trigger_time - now).num_seconds().max(0) as u64;

            tray::update_tray_tooltip(&format!(
                "Meet Pounce - 次: {} ({})",
                mtg.start_time.format("%H:%M"),
                mtg.title
            ));

            tokio::select! {
                _ = tokio::time::sleep(tokio::time::Duration::from_secs(wait_secs)) => {
                    notified_ids.insert(mtg.id.clone(), mtg.start_time);
                    let _ = notification::show_meeting_toast(
                        &mtg.title,
                        mtg.service_type,
                        mtg.meeting_url.as_deref(),
                        mins,
                        &sound,
                        vol,
                    );
                    trim_working_set();
                }
                _ = reschedule_rx.changed() => {}
            }
        } else {
            let tip = if is_configured {
                "Meet Pounce - 本日の残り予定なし"
            } else {
                "Meet Pounce - 初期設定が必要です (右クリックで設定)"
            };
            tray::update_tray_tooltip(tip);
            let _ = reschedule_rx.changed().await;
        }
    }
}
