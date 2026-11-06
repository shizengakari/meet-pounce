use crate::config::{is_startup_enabled, set_startup, AppConfig};
use crate::notification;
use std::collections::HashMap;
use std::error::Error;
use tiny_http::{Header, Response, Server};
use url::Url;

/// ブラウザ上でローカル設定画面を開き、設定完了まで待機する
pub async fn run_settings_server() -> Result<(), Box<dyn Error + Send + Sync>> {
    let port = 8989;
    let server = Server::http(format!("127.0.0.1:{port}"))
        .map_err(|e| format!("ポート {port} のバインドに失敗しました: {e}"))?;

    let url = format!("http://127.0.0.1:{port}");
    println!("ブラウザで設定画面を開きます: {url}");
    let _ = open::that(&url);

    loop {
        let request = server.recv()?;
        let req_url = format!("http://localhost{}", request.url());
        let parsed = Url::parse(&req_url)?;
        let path = parsed.path();

        if path == "/" {
            let cfg = AppConfig::load();
            let startup = is_startup_enabled();
            let html = render_settings_html(&cfg, startup);
            let resp = Response::from_string(html).with_header(
                Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..]).unwrap(),
            );
            let _ = request.respond(resp);
        } else if path == "/save" {
            let mut cfg = AppConfig::load();
            let query: HashMap<_, _> = parsed.query_pairs().collect();

            if let Some(cid) = query.get("client_id") {
                cfg.client_id = cid.trim().to_string();
            }
            if let Some(csec) = query.get("client_secret") {
                cfg.client_secret = csec.trim().to_string();
            }
            if let Some(mins) = query.get("notify_minutes") {
                if let Ok(m) = mins.parse::<u32>() {
                    cfg.notify_minutes_before = m;
                }
            }
            if let Some(snd) = query.get("sound_type") {
                cfg.sound_type = snd.to_string();
            }

            let _ = cfg.save();

            let enable_startup = query.get("startup").map(|v| v == "1").unwrap_or(false);
            let _ = set_startup(enable_startup);

            let html = r#"<!DOCTYPE html><html><head><meta charset="utf-8">
                <title>設定を保存しました</title>
                <script>
                    window.close();
                </script>
                <style>
                    body { background: #0B0F17; color: #F8FAFC; font-family: 'Segoe UI', -apple-system, sans-serif; display: flex; align-items: center; justify-content: center; height: 100vh; margin: 0; }
                    .card { background: #111827; border: 1px solid #1E293B; border-radius: 8px; padding: 28px; text-align: center; max-width: 360px; }
                    h2 { color: #10B981; font-size: 16px; margin: 0 0 8px 0; }
                    p { color: #94A3B8; font-size: 13px; margin: 0; }
                </style></head><body><div class="card"><h2>設定を保存しました</h2><p>このタブを閉じてください</p></div></body></html>"#;
            let resp = Response::from_string(html).with_header(
                Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..]).unwrap(),
            );
            let _ = request.respond(resp);
            break;
        } else if path == "/test-sound" {
            let query: HashMap<_, _> = parsed.query_pairs().collect();
            let sound_type = query.get("sound").map(|s| s.as_ref()).unwrap_or("calendar");
            let _ = notification::show_meeting_toast(
                "テストミーティング",
                "Meet",
                Some("https://meet.google.com/test"),
                1,
                sound_type,
            );
            let _ = request.respond(Response::from_string("OK"));
        } else if path == "/add-account" {
            let cfg = AppConfig::load();
            if cfg.client_id.is_empty() || cfg.client_secret.is_empty() {
                let html = "<script>alert('先にClient IDとClient Secretを保存してください。'); window.location.href='/';</script>";
                let _ = request.respond(Response::from_string(html).with_header(
                    Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..]).unwrap(),
                ));
            } else {
                let auth_url = format!(
                    "https://accounts.google.com/o/oauth2/v2/auth?client_id={}&redirect_uri=http://127.0.0.1:{port}/oauth-callback&response_type=code&scope=https://www.googleapis.com/auth/calendar.readonly https://www.googleapis.com/auth/userinfo.email&access_type=offline&prompt=consent",
                    url::form_urlencoded::byte_serialize(cfg.client_id.as_bytes()).collect::<String>()
                );
                let html = format!("<script>window.location.href='{auth_url}';</script>");
                let _ = request.respond(Response::from_string(html).with_header(
                    Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..]).unwrap(),
                ));
            }
        } else if path == "/oauth-callback" {
            let query: HashMap<_, _> = parsed.query_pairs().collect();
            if let Some(code) = query.get("code") {
                let mut cfg = AppConfig::load();
                let redirect_uri = format!("http://127.0.0.1:{port}/oauth-callback");

                let client = reqwest::Client::new();
                let mut params = HashMap::new();
                params.insert("client_id", cfg.client_id.as_str());
                params.insert("client_secret", cfg.client_secret.as_str());
                params.insert("code", code.as_ref());
                params.insert("grant_type", "authorization_code");
                params.insert("redirect_uri", redirect_uri.as_str());

                if let Ok(res) = client.post("https://oauth2.googleapis.com/token").form(&params).send().await {
                    if let Ok(token_json) = res.json::<serde_json::Value>().await {
                        if let Some(refresh_token) = token_json.get("refresh_token").and_then(|v| v.as_str()) {
                            let access_token = token_json.get("access_token").and_then(|v| v.as_str()).unwrap_or("");
                            if let Ok(ures) = client.get("https://www.googleapis.com/oauth2/v2/userinfo").bearer_auth(access_token).send().await {
                                if let Ok(ujson) = ures.json::<serde_json::Value>().await {
                                    if let Some(email) = ujson.get("email").and_then(|v| v.as_str()) {
                                        cfg.add_or_update_account(email.to_string(), refresh_token.to_string());
                                    }
                                }
                            }
                        }
                    }
                }
            }
            let html = "<script>alert('アカウントが追加されました。'); window.location.href='/';</script>";
            let _ = request.respond(Response::from_string(html).with_header(
                Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..]).unwrap(),
            ));
        } else if path == "/remove-account" {
            let query: HashMap<_, _> = parsed.query_pairs().collect();
            if let Some(email) = query.get("email") {
                let mut cfg = AppConfig::load();
                cfg.remove_account(email);
            }
            let html = "<script>window.location.href='/';</script>";
            let _ = request.respond(Response::from_string(html).with_header(
                Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=utf-8"[..]).unwrap(),
            ));
        }
    }

    Ok(())
}

fn render_settings_html(cfg: &AppConfig, startup_enabled: bool) -> String {
    let mut acc_rows = String::new();
    for a in &cfg.accounts {
        acc_rows.push_str(&format!(
            r#"<div class="row" style="margin-bottom:8px;">
                <span style="font-size:13px;color:#F8FAFC;">{}</span>
                <a href="/remove-account?email={}" class="btn-danger" style="text-decoration:none;padding:4px 8px;font-size:11px;">解除</a>
            </div>"#,
            a.email, a.email
        ));
    }
    if acc_rows.is_empty() {
        acc_rows = "<p style='color:#64748B;font-size:12px;margin:4px 0 8px 0;'>連携アカウントはありません</p>".to_string();
    }

    format!(
        r#"<!DOCTYPE html>
<html lang="ja">
<head>
<meta charset="utf-8">
<title>Meet Pounce 設定</title>
<style>
  body {{ background: #0B0F17; color: #F8FAFC; font-family: 'Segoe UI', -apple-system, sans-serif; display: flex; justify-content: center; padding: 40px 20px; margin: 0; }}
  .container {{ width: 100%; max-width: 520px; }}
  .title {{ font-size: 16px; font-weight: 700; color: #F8FAFC; margin: 0 0 20px 0; letter-spacing: 0.5px; }}
  .card {{ background: #111827; border: 1px solid #1E293B; border-radius: 8px; padding: 18px; margin-bottom: 14px; }}
  .card-title {{ font-size: 11px; font-weight: 700; color: #64748B; letter-spacing: 0.5px; text-transform: uppercase; margin-bottom: 12px; }}
  .guide-box {{ background: #0F172A; border: 1px solid #1E293B; border-radius: 6px; padding: 12px 14px; margin-bottom: 14px; }}
  .guide-header {{ font-size: 12px; font-weight: 600; color: #60A5FA; margin-bottom: 8px; display: flex; justify-content: space-between; align-items: center; }}
  .guide-steps {{ margin: 0; padding-left: 18px; font-size: 12px; color: #94A3B8; line-height: 1.6; }}
  .guide-steps li {{ margin-bottom: 4px; }}
  .guide-link {{ color: #38BDF8; text-decoration: none; }}
  .guide-link:hover {{ text-decoration: underline; }}
  .field {{ margin-bottom: 12px; }}
  .field:last-child {{ margin-bottom: 0; }}
  .field label {{ display: block; font-size: 12px; color: #CBD5E1; margin-bottom: 6px; font-weight: 500; }}
  input[type="text"], select {{ width: 100%; box-sizing: border-box; background: #0B0F17; border: 1px solid #1F2937; color: #F8FAFC; padding: 8px 12px; border-radius: 6px; font-size: 13px; outline: none; }}
  input[type="text"]:focus, select:focus {{ border-color: #2563EB; }}
  .row {{ display: flex; justify-content: space-between; align-items: center; }}
  .btn {{ background: #2563EB; color: #FFFFFF; border: none; border-radius: 6px; padding: 10px 16px; font-size: 13px; font-weight: 600; cursor: pointer; text-decoration: none; display: inline-block; }}
  .btn:hover {{ background: #1D4ED8; }}
  .btn-secondary {{ background: #1E293B; color: #CBD5E1; padding: 6px 12px; font-size: 12px; border-radius: 6px; text-decoration: none; border: 1px solid #334155; display: inline-block; }}
  .btn-secondary:hover {{ background: #334155; }}
  .btn-danger {{ background: #7F1D1D; color: #FCA5A5; border-radius: 4px; border: none; }}
  .btn-danger:hover {{ background: #991B1B; }}
  .checkbox-label {{ display: flex; align-items: center; gap: 8px; font-size: 13px; color: #CBD5E1; cursor: pointer; }}
</style>
</head>
<body>
<div class="container">
  <div class="title">MEET POUNCE 設定</div>

  <form action="/save" method="GET">
    <!-- Google OAuth -->
    <div class="card">
      <div class="card-title">Google 連携設定</div>

      <div class="guide-box">
        <div class="guide-header">
          <span>初期設定の手順</span>
          <a href="https://console.cloud.google.com/apis/credentials" target="_blank" class="guide-link" style="font-size:11px;">Google Cloud 認証情報画面 ↗</a>
        </div>
        <ol class="guide-steps">
          <li>上記のリンクから Google Cloud にアクセスし、プロジェクトを作成します。</li>
          <li>「ライブラリ」から <strong>Google Calendar API</strong> を有効にします。</li>
          <li>「認証情報を作成」→「OAuth クライアント ID」（種類: <strong>デスクトップ アプリ</strong>）を作成します。</li>
          <li>発行された Client ID と Client Secret を下の欄に入力してください。</li>
        </ol>
      </div>

      <div class="field">
        <label>Client ID</label>
        <input type="text" name="client_id" value="{}" placeholder="xxxx.apps.googleusercontent.com" required>
      </div>
      <div class="field">
        <label>Client Secret</label>
        <input type="text" name="client_secret" value="{}" placeholder="GOCSPX-xxxx" required>
      </div>
    </div>

    <!-- 連携アカウント -->
    <div class="card">
      <div class="card-title">連携アカウント</div>
      {}
      <div style="margin-top: 10px;">
        <a href="/add-account" class="btn-secondary">+ Google アカウントを追加</a>
      </div>
    </div>

    <!-- 通知 & サウンド設定 -->
    <div class="card">
      <div class="card-title">通知設定</div>
      <div class="field">
        <label>通知する時間</label>
        <select name="notify_minutes">
          <option value="1" {}>会議の 1 分前</option>
          <option value="2" {}>会議の 2 分前</option>
          <option value="3" {}>会議の 3 分前</option>
          <option value="5" {}>会議の 5 分前</option>
        </select>
      </div>
      <div class="field">
        <div class="row">
          <label style="margin:0;">通知サウンド</label>
          <button type="button" class="btn-secondary" onclick="testSound()" style="padding:2px 8px;font-size:11px;">試聴テスト</button>
        </div>
        <select id="sound_type" name="sound_type" style="margin-top:6px;">
          <option value="calendar" {}>カレンダー通知音</option>
          <option value="chimes" {}>チャイム音</option>
          <option value="ring" {}>着信音</option>
          <option value="alarm" {}>アラーム音</option>
          <option value="generic" {}>Windows標準通知音</option>
          <option value="silent" {}>無音</option>
        </select>
      </div>
    </div>

    <!-- システム設定 -->
    <div class="card">
      <div class="card-title">システム設定</div>
      <label class="checkbox-label">
        <input type="checkbox" name="startup" value="1" {}>
        <span>Windows起動時に自動起動する</span>
      </label>
    </div>

    <div style="margin-top: 20px;">
      <button type="submit" class="btn" style="width: 100%;">設定を保存</button>
    </div>
  </form>
</div>

<script>
function testSound() {{
  const sound = document.getElementById('sound_type').value;
  fetch('/test-sound?sound=' + sound);
}}
</script>
</body>
</html>"#,
        cfg.client_id,
        cfg.client_secret,
        acc_rows,
        if cfg.notify_minutes_before == 1 { "selected" } else { "" },
        if cfg.notify_minutes_before == 2 { "selected" } else { "" },
        if cfg.notify_minutes_before == 3 { "selected" } else { "" },
        if cfg.notify_minutes_before == 5 { "selected" } else { "" },
        if cfg.sound_type == "calendar" { "selected" } else { "" },
        if cfg.sound_type == "chimes" { "selected" } else { "" },
        if cfg.sound_type == "ring" { "selected" } else { "" },
        if cfg.sound_type == "alarm" { "selected" } else { "" },
        if cfg.sound_type == "generic" { "selected" } else { "" },
        if cfg.sound_type == "silent" { "selected" } else { "" },
        if startup_enabled { "checked" } else { "" }
    )
}
