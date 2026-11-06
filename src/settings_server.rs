use crate::config::{is_startup_enabled, set_startup, AppConfig};
use crate::notification;
use crate::tray::open_browser;
use std::collections::HashMap;
use std::error::Error;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::time::timeout;

fn url_decode(s: &str) -> String {
    let mut out = Vec::with_capacity(s.len());
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'+' {
            out.push(b' ');
            i += 1;
        } else if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = ((b[i + 1] as char).to_digit(16), (b[i + 2] as char).to_digit(16)) {
                out.push((h * 16 + l) as u8);
                i += 3;
                continue;
            }
            out.push(b[i]);
            i += 1;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn url_encode(s: &str) -> String {
    s.bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

fn parse_query(q: &str) -> HashMap<String, String> {
    q.split('&')
        .filter(|s| !s.is_empty())
        .map(|pair| {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            (url_decode(k), url_decode(v))
        })
        .collect()
}

async fn send_http(stream: &mut tokio::net::TcpStream, content_type: &str, body: &str) {
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes()).await;
    let _ = stream.write_all(body.as_bytes()).await;
    let _ = stream.flush().await;
}

async fn exchange_oauth_code(port: u16, code: &str, cfg: &mut AppConfig) -> Option<()> {
    let redirect_uri = format!("http://127.0.0.1:{port}/oauth-callback");
    let client = reqwest::Client::new();
    let params = [
        ("client_id", cfg.client_id.as_str()),
        ("client_secret", cfg.client_secret.as_str()),
        ("code", code),
        ("grant_type", "authorization_code"),
        ("redirect_uri", redirect_uri.as_str()),
    ];

    let res = client.post("https://oauth2.googleapis.com/token").form(&params).send().await.ok()?;
    let tjson = res.json::<serde_json::Value>().await.ok()?;
    let rt = tjson.get("refresh_token")?.as_str()?;
    let at = tjson.get("access_token").and_then(|v| v.as_str()).unwrap_or("");

    let ures = client.get("https://www.googleapis.com/oauth2/v2/userinfo").bearer_auth(at).send().await.ok()?;
    let ujson = ures.json::<serde_json::Value>().await.ok()?;
    let email = ujson.get("email")?.as_str()?;

    cfg.add_or_update_account(email.to_string(), rt.to_string());
    Some(())
}

/// ブラウザ上でローカル設定画面を開き、保存またはタブ終了まで待機する
pub async fn run_settings_server() -> Result<(), Box<dyn Error + Send + Sync>> {
    let port = 8989;
    let url = format!("http://127.0.0.1:{port}");
    let Ok(listener) = TcpListener::bind(format!("127.0.0.1:{port}")).await else {
        open_browser(&url);
        return Ok(());
    };
    open_browser(&url);

    let mut buf = [0u8; 4096];
    let mut wait_timeout = Duration::from_secs(600);

    while let Ok(Ok((mut stream, _))) = timeout(wait_timeout, listener.accept()).await {
        wait_timeout = Duration::from_secs(600);
        let Ok(n) = stream.read(&mut buf).await else { continue };
        if n == 0 {
            continue;
        }
        let req_str = String::from_utf8_lossy(&buf[..n]);
        let Some(first_line) = req_str.lines().next() else { continue };
        let Some(target) = first_line.split_whitespace().nth(1) else { continue };
        let (path, query_str) = target.split_once('?').unwrap_or((target, ""));
        let query = parse_query(query_str);

        match path {
            "/" => {
                let cfg = AppConfig::load();
                let html = render_settings_html(&cfg, is_startup_enabled());
                send_http(&mut stream, "text/html; charset=utf-8", &html).await;
            }
            "/save" => {
                let mut cfg = AppConfig::load();

                if let Some(cid) = query.get("client_id") {
                    cfg.client_id = cid.trim().to_string();
                }
                if let Some(csec) = query.get("client_secret") {
                    cfg.client_secret = csec.trim().to_string();
                }

                // オンライン会議の設定
                cfg.meeting_enabled = query.get("meeting_enabled").map(|v| v == "1").unwrap_or(false);
                if let Some(m) = query.get("notify_minutes").and_then(|v| v.parse::<u32>().ok()) {
                    cfg.notify_minutes_before = m.clamp(0, 120);
                }
                if let Some(s) = query.get("sound_type") {
                    cfg.sound_type = s.to_string();
                }
                if let Some(v) = query.get("sound_volume").and_then(|v| v.parse::<u32>().ok()) {
                    cfg.sound_volume = v.clamp(0, 500);
                }

                // 通常の予定の設定
                cfg.event_enabled = query.get("event_enabled").map(|v| v == "1").unwrap_or(false);
                if let Some(m) = query.get("event_notify_minutes").and_then(|v| v.parse::<u32>().ok()) {
                    cfg.event_notify_minutes_before = m.clamp(0, 120);
                }
                if let Some(s) = query.get("event_sound_type") {
                    cfg.event_sound_type = s.to_string();
                }
                if let Some(v) = query.get("event_sound_volume").and_then(|v| v.parse::<u32>().ok()) {
                    cfg.event_sound_volume = v.clamp(0, 500);
                }

                let _ = cfg.save();
                let _ = set_startup(query.get("startup").map(|v| v == "1").unwrap_or(false));

                send_http(&mut stream, "text/plain", "OK").await;
                break;
            }
            "/close" => {
                send_http(&mut stream, "text/plain", "OK").await;
                // リロードでなければ1.5秒後にサーバーを終了する
                wait_timeout = Duration::from_millis(1500);
            }
            "/test-sound" => {
                let sound = query.get("sound").map(|s| s.as_str()).unwrap_or("calendar");
                let vol = query.get("volume").and_then(|v| v.parse::<u32>().ok()).unwrap_or(200).clamp(0, 500);
                let mins = query.get("mins").and_then(|m| m.parse::<i64>().ok()).unwrap_or(1).clamp(0, 120);
                let is_event = query.get("kind").map(|k| k == "event").unwrap_or(false);
                let (title, service, url) = if is_event {
                    ("テスト予定", "予定", None)
                } else {
                    ("テストミーティング", "Meet", Some("https://meet.google.com"))
                };
                let _ = notification::show_meeting_toast(title, service, url, mins, sound, vol);
                send_http(&mut stream, "text/plain", "OK").await;
            }
            "/add-account" => {
                let mut cfg = AppConfig::load();
                if let Some(cid) = query.get("client_id").filter(|s| !s.trim().is_empty()) {
                    cfg.client_id = cid.trim().to_string();
                }
                if let Some(csec) = query.get("client_secret").filter(|s| !s.trim().is_empty()) {
                    cfg.client_secret = csec.trim().to_string();
                }
                let _ = cfg.save();

                let html = if cfg.client_id.is_empty() || cfg.client_secret.is_empty() {
                    "<script>alert('先にClient IDとClient Secretを入力してください。'); window.close();</script>".to_string()
                } else {
                    let cid = url_encode(&cfg.client_id);
                    format!(
                        "<script>location.replace('https://accounts.google.com/o/oauth2/v2/auth?client_id={cid}&redirect_uri=http://127.0.0.1:{port}/oauth-callback&response_type=code&scope=https://www.googleapis.com/auth/calendar.readonly%20https://www.googleapis.com/auth/userinfo.email&access_type=offline&prompt=consent');</script>"
                    )
                };
                send_http(&mut stream, "text/html; charset=utf-8", &html).await;
            }
            "/oauth-callback" => {
                if let Some(code) = query.get("code") {
                    let mut cfg = AppConfig::load();
                    let _ = exchange_oauth_code(port, code, &mut cfg).await;
                }
                send_http(
                    &mut stream,
                    "text/html; charset=utf-8",
                    "<script>if(window.opener){window.opener.reloadPage();window.close();}else{location.replace('/');}</script>",
                )
                .await;
            }
            "/remove-account" => {
                if let Some(email) = query.get("email") {
                    let mut cfg = AppConfig::load();
                    cfg.remove_account(email);
                }
                send_http(&mut stream, "text/plain", "OK").await;
            }
            _ => {
                send_http(&mut stream, "text/plain", "Not Found").await;
            }
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn render_notify_card(
    title: &str,
    enable_name: &str,
    enabled: bool,
    mins_name: &str,
    mins_val: u32,
    sound_name: &str,
    sound_val: &str,
    vol_name: &str,
    vol_val: u32,
    kind: &str,
) -> String {
    let checked = if enabled { "checked" } else { "" };
    let sound_opts: String = [
        ("calendar", "カレンダー通知音"),
        ("chimes", "チャイム音"),
        ("ring", "着信音"),
        ("alarm", "アラーム音"),
        ("generic", "Windows標準通知音"),
        ("silent", "無音"),
    ]
    .iter()
    .map(|(val, label)| {
        let sel = if *val == sound_val { " selected" } else { "" };
        format!(r#"<option value="{val}"{sel}>{label}</option>"#)
    })
    .collect();

    format!(
        r#"<div class="card">
      <div class="row" style="margin-bottom:14px;">
        <span class="card-title" style="margin:0;">{title}</span>
        <label class="checkbox-label" style="color:#60A5FA;font-weight:600;">
          <input type="checkbox" name="{enable_name}" value="1" {checked}>
          <span>通知 ON</span>
        </label>
      </div>
      <div class="grid-2">
        <div class="field">
          <label>通知タイミング <span class="hint">(0〜120)</span></label>
          <div class="input-unit">
            <input type="number" id="{mins_name}" name="{mins_name}" value="{mins_val}" min="0" max="120" required>
            <span>分前</span>
          </div>
        </div>
        <div class="field">
          <label>音量ブースト <span class="hint">(0〜500%)</span></label>
          <div class="input-unit">
            <input type="number" id="{vol_name}" name="{vol_name}" value="{vol_val}" min="0" max="500" required>
            <span>%</span>
          </div>
        </div>
      </div>
      <div class="field" style="margin-top:12px;">
        <div class="row" style="margin-bottom:6px;">
          <label style="margin:0;">通知サウンド</label>
          <button type="button" class="btn-secondary" onclick="testSound('{sound_name}','{vol_name}','{mins_name}','{kind}')" style="padding:3px 10px;font-size:12px;">試聴テスト</button>
        </div>
        <select id="{sound_name}" name="{sound_name}">{sound_opts}</select>
      </div>
    </div>"#
    )
}

fn render_settings_html(cfg: &AppConfig, startup_enabled: bool) -> String {
    let acc_rows = if cfg.accounts.is_empty() {
        "<p style='color:#64748B;font-size:13px;margin:4px 0 10px 0;'>連携アカウントはありません</p>".to_string()
    } else {
        cfg.accounts
            .iter()
            .map(|a| {
                format!(
                    r#"<div class="row" style="margin-bottom:8px;">
                        <span style="font-size:13.5px;color:#F8FAFC;">{}</span>
                        <button type="button" onclick="removeAccount('{}')" class="btn-danger" style="padding:4px 10px;font-size:12px;cursor:pointer;font-family:inherit;">解除</button>
                    </div>"#,
                    a.email, a.email
                )
            })
            .collect()
    };

    let meeting_card = render_notify_card(
        "オンライン会議 (Meet / Zoom / Teams 等)",
        "meeting_enabled",
        cfg.meeting_enabled,
        "notify_minutes",
        cfg.notify_minutes_before,
        "sound_type",
        &cfg.sound_type,
        "sound_volume",
        cfg.sound_volume,
        "meeting",
    );

    let event_card = render_notify_card(
        "通常の予定 (URLなし)",
        "event_enabled",
        cfg.event_enabled,
        "event_notify_minutes",
        cfg.event_notify_minutes_before,
        "event_sound_type",
        &cfg.event_sound_type,
        "event_sound_volume",
        cfg.event_sound_volume,
        "event",
    );

    let startup_checked = if startup_enabled { "checked" } else { "" };

    format!(
        r#"<!DOCTYPE html>
<html lang="ja">
<head>
<meta charset="utf-8">
<title>Meet Pounce 設定</title>
<style>
  body {{ background:#0B0F17; color:#F8FAFC; font-family:'Segoe UI','BIZ UDPGothic','Yu Gothic UI',Meiryo,sans-serif; display:flex; justify-content:center; padding:36px 16px; margin:0; line-height:1.5; }}
  .container {{ width:100%; max-width:500px; }}
  .title {{ font-size:17px; font-weight:700; color:#F8FAFC; margin:0 0 18px 0; letter-spacing:0.4px; }}
  .card {{ background:#111827; border:1px solid #1E293B; border-radius:10px; padding:18px 20px; margin-bottom:14px; }}
  .card-title {{ font-size:13px; font-weight:700; color:#E2E8F0; }}
  .guide-box {{ background:#0F172A; border:1px solid #1E293B; border-radius:8px; padding:12px 14px; margin-bottom:14px; }}
  .guide-steps {{ margin:0; padding-left:18px; font-size:12.5px; color:#CBD5E1; line-height:1.65; }}
  .guide-steps li {{ margin-bottom:3px; }}
  .guide-link {{ color:#38BDF8; font-weight:600; text-decoration:none; }}
  .guide-link:hover {{ text-decoration:underline; }}
  .grid-2 {{ display:grid; grid-template-columns:1fr 1fr; gap:12px; }}
  .field {{ margin-bottom:12px; }}
  .field:last-child {{ margin-bottom:0; }}
  .field label {{ display:block; font-size:12.5px; color:#CBD5E1; margin-bottom:6px; font-weight:500; }}
  .hint {{ color:#64748B; font-size:11.5px; font-weight:400; }}
  input[type="text"], input[type="number"], select {{ width:100%; box-sizing:border-box; background:#0B0F17; border:1px solid #1F2937; color:#F8FAFC; padding:8px 12px; border-radius:6px; font-size:13.5px; font-family:inherit; outline:none; }}
  input[type="text"]:focus, input[type="number"]:focus, select:focus {{ border-color:#3B82F6; }}
  .input-unit {{ display:flex; align-items:center; background:#0B0F17; border:1px solid #1F2937; border-radius:6px; padding-right:10px; }}
  .input-unit:focus-within {{ border-color:#3B82F6; }}
  .input-unit input {{ border:none; background:transparent; padding-right:4px; }}
  .input-unit span {{ color:#94A3B8; font-size:12.5px; white-space:nowrap; user-select:none; }}
  .row {{ display:flex; justify-content:space-between; align-items:center; }}
  .btn {{ background:#2563EB; color:#FFFFFF; border:none; border-radius:8px; padding:11px 16px; font-size:14px; font-weight:600; font-family:inherit; cursor:pointer; width:100%; }}
  .btn:hover {{ background:#1D4ED8; }}
  .btn-secondary {{ background:#1E293B; color:#E2E8F0; padding:6px 12px; font-size:12.5px; border-radius:6px; text-decoration:none; border:1px solid #334155; display:inline-block; cursor:pointer; font-family:inherit; }}
  .btn-secondary:hover {{ background:#334155; }}
  .btn-danger {{ background:#7F1D1D; color:#FCA5A5; border-radius:5px; border:none; }}
  .btn-danger:hover {{ background:#991B1B; }}
  .checkbox-label {{ display:flex; align-items:center; gap:7px; font-size:13px; color:#E2E8F0; cursor:pointer; user-select:none; }}
</style>
</head>
<body>
<div class="container">
  <div class="title">Meet Pounce 設定</div>

  <form onsubmit="saveSettings(event)">
    <div class="card">
      <div class="card-title" style="margin-bottom:10px;">Google カレンダー連携</div>
      <div class="guide-box">
        <ol class="guide-steps">
          <li><a href="https://console.cloud.google.com/apis/credentials" target="_blank" class="guide-link">Google Cloud ↗</a> でプロジェクトを作成し、<strong>Google Calendar API</strong> を有効化</li>
          <li>「認証情報を作成」→「OAuth クライアント ID」（種類: <strong>デスクトップ アプリ</strong>）を作成</li>
          <li>発行された Client ID と Client Secret を下に入力</li>
        </ol>
      </div>
      <div class="field">
        <label>Client ID</label>
        <input type="text" id="client_id" name="client_id" value="{}" placeholder="xxxx.apps.googleusercontent.com" required>
      </div>
      <div class="field">
        <label>Client Secret</label>
        <input type="text" id="client_secret" name="client_secret" value="{}" placeholder="GOCSPX-xxxx" required>
      </div>
      <div style="margin-top:14px;padding-top:12px;border-top:1px solid #1E293B;">
        <label style="display:block;font-size:12.5px;color:#CBD5E1;margin-bottom:6px;font-weight:500;">連携済みアカウント</label>
        {acc_rows}
        <button type="button" onclick="addAccount()" class="btn-secondary">+ Google アカウントを追加</button>
      </div>
    </div>

    {meeting_card}
    {event_card}

    <div class="card">
      <label class="checkbox-label">
        <input type="checkbox" name="startup" value="1" {startup_checked}>
        <span>Windows 起動時に自動起動する</span>
      </label>
    </div>

    <button type="submit" id="save_btn" class="btn">設定を保存</button>
  </form>
</div>

<script>
let reloading = false;
window.addEventListener('pagehide', () => {{
  if (!reloading) navigator.sendBeacon('/close');
}});
function reloadPage() {{
  reloading = true;
  location.replace('/');
}}
function addAccount() {{
  const cid = encodeURIComponent(document.getElementById('client_id').value.trim());
  const csec = encodeURIComponent(document.getElementById('client_secret').value.trim());
  window.open('/add-account?client_id=' + cid + '&client_secret=' + csec, 'oauth', 'width=520,height=640');
}}
async function removeAccount(email) {{
  await fetch('/remove-account?email=' + encodeURIComponent(email));
  reloadPage();
}}
async function saveSettings(e) {{
  e.preventDefault();
  reloading = true;
  const q = new URLSearchParams(new FormData(e.target)).toString();
  await fetch('/save?' + q);
  window.open('', '_self').close();
  window.close();
}}
function testSound(soundId, volId, minsId, kind) {{
  const sound = document.getElementById(soundId).value;
  const vol = document.getElementById(volId).value;
  const mins = document.getElementById(minsId).value;
  fetch('/test-sound?sound=' + sound + '&volume=' + vol + '&mins=' + mins + '&kind=' + kind);
}}
</script>
</body>
</html>"#,
        cfg.client_id, cfg.client_secret
    )
}
