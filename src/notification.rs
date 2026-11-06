use std::os::windows::ffi::OsStrExt;
use std::process::Command;
use windows::{
    core::{HSTRING, Result},
    Data::Xml::Dom::XmlDocument,
    UI::Notifications::{ToastNotification, ToastNotificationManager},
};

const APP_ID: &str = "MeetPounce";
const APP_DISPLAY_NAME: &str = "Meet Pounce";

// winmm.dll の PlaySoundW API (大音量で確実に音を鳴らす)
#[link(name = "winmm")]
extern "system" {
    fn PlaySoundW(pszSound: *const u16, hmod: usize, fdwSound: u32) -> i32;
}

const SND_ASYNC: u32 = 0x0001;
const SND_FILENAME: u32 = 0x00020000;

/// Windowsレジストリにアプリ名 "Meet Pounce" を登録する (通知元をPowerShellではなくMeet Pounceにする)
pub fn ensure_app_registered() {
    let _ = Command::new("reg")
        .args([
            "add",
            &format!("HKCU\\Software\\Classes\\AppUserModelId\\{APP_ID}"),
            "/v",
            "DisplayName",
            "/t",
            "REG_SZ",
            "/d",
            APP_DISPLAY_NAME,
            "/f",
        ])
        .output();

    let _ = Command::new("reg")
        .args([
            "add",
            &format!("HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Notifications\\Settings\\{APP_ID}"),
            "/v",
            "ShowInActionCenter",
            "/t",
            "REG_DWORD",
            "/d",
            "1",
            "/f",
        ])
        .output();

    let _ = Command::new("reg")
        .args([
            "add",
            &format!("HKCU\\Software\\Microsoft\\Windows\\CurrentVersion\\Notifications\\Settings\\{APP_ID}"),
            "/v",
            "Enabled",
            "/t",
            "REG_DWORD",
            "/d",
            "1",
            "/f",
        ])
        .output();
}

/// 通知サウンドのファイル名を取得する（silent の場合は None）
pub fn resolve_sound_file(sound_type: &str) -> Option<&'static str> {
    match sound_type {
        "silent" => None,
        "chimes" => Some("chimes.wav"),
        "ring" => Some("Ring01.wav"),
        "alarm" => Some("Alarm01.wav"),
        "generic" => Some("Windows Notify System Generic.wav"),
        _ => Some("Windows Notify Calendar.wav"),
    }
}

/// 残り時間に応じた表示文言を生成する
pub fn format_time_text(minutes_left: i64) -> String {
    if minutes_left <= 0 {
        "まもなく始まります".to_string()
    } else {
        format!("{minutes_left}分後に開始します")
    }
}

/// サービス短縮名を正式名称に変換する
pub fn format_service_name(service: &str) -> &str {
    match service {
        "Meet" => "Google Meet",
        "Zoom" => "Zoom",
        "Teams" => "Microsoft Teams",
        "Webex" => "Cisco Webex",
        _ => service,
    }
}

/// トースト通知用 XML 文字列を組み立てる
pub fn build_toast_xml(
    title: &str,
    service: &str,
    meeting_url: Option<&str>,
    minutes_left: i64,
) -> String {
    let time_text = format_time_text(minutes_left);
    let full_service = format_service_name(service);

    let title_escaped = xml_escape(title);
    let service_escaped = xml_escape(full_service);

    let action_xml = if let Some(url) = meeting_url {
        let url_escaped = xml_escape(url);
        format!(r#"<action content="参加する" activationType="protocol" arguments="{url_escaped}" />"#)
    } else {
        String::new()
    };

    format!(
        r#"<toast scenario="reminder">
            <visual>
                <binding template="ToastGeneric">
                    <text>{title_escaped}</text>
                    <text>{service_escaped} ・ {time_text}</text>
                </binding>
            </visual>
            <actions>
                {action_xml}
            </actions>
            <audio silent="true" />
        </toast>"#
    )
}

/// Windowsの標準WAVファイルを大音量・明瞭に再生する
pub fn play_sound(sound_type: &str) {
    if let Some(file_name) = resolve_sound_file(sound_type) {
        let path = format!(r"C:\Windows\Media\{file_name}");
        let wide: Vec<u16> = std::ffi::OsStr::new(&path)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();

        unsafe {
            PlaySoundW(wide.as_ptr(), 0, SND_ASYNC | SND_FILENAME);
        }
    }
}

/// 会議のトースト通知をWindowsネイティブで表示する
/// - 発信元名: "Meet Pounce"
/// - 1行目: イベント名 (太字)
/// - 2行目: サービス名 (Google Meet, Zoom等) ・ 残り時間
/// - scenario="reminder" でユーザーが操作するまで自動で消えない
/// - 「参加する」ボタンのみを表示
pub fn show_meeting_toast(
    title: &str,
    service: &str,
    meeting_url: Option<&str>,
    minutes_left: i64,
    sound_type: &str,
) -> Result<()> {
    ensure_app_registered();

    let xml = build_toast_xml(title, service, meeting_url, minutes_left);

    let doc = XmlDocument::new()?;
    doc.LoadXml(&HSTRING::from(xml))?;

    let notification = ToastNotification::CreateToastNotification(&doc)?;

    // アプリID "MeetPounce" でトーストを表示 (表示名: Meet Pounce)
    let notifier = ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(APP_ID))
        .or_else(|_| ToastNotificationManager::CreateToastNotifier())?;

    notifier.Show(&notification)?;

    // 音声を大音量再生
    play_sound(sound_type);

    Ok(())
}

pub fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_xml_escape() {
        assert_eq!(xml_escape("Tom & Jerry"), "Tom &amp; Jerry");
        assert_eq!(xml_escape("<meeting>"), "&lt;meeting&gt;");
        assert_eq!(xml_escape(r#"say "hello" & 'bye'"#), "say &quot;hello&quot; &amp; &apos;bye&apos;");
        assert_eq!(xml_escape("regular text"), "regular text");
    }

    #[test]
    fn test_resolve_sound_file() {
        assert_eq!(resolve_sound_file("silent"), None);
        assert_eq!(resolve_sound_file("chimes"), Some("chimes.wav"));
        assert_eq!(resolve_sound_file("ring"), Some("Ring01.wav"));
        assert_eq!(resolve_sound_file("alarm"), Some("Alarm01.wav"));
        assert_eq!(resolve_sound_file("generic"), Some("Windows Notify System Generic.wav"));
        assert_eq!(resolve_sound_file("calendar"), Some("Windows Notify Calendar.wav"));
        assert_eq!(resolve_sound_file("unknown_value"), Some("Windows Notify Calendar.wav"));
    }

    #[test]
    fn test_format_time_text() {
        assert_eq!(format_time_text(0), "まもなく始まります");
        assert_eq!(format_time_text(-1), "まもなく始まります");
        assert_eq!(format_time_text(1), "1分後に開始します");
        assert_eq!(format_time_text(5), "5分後に開始します");
    }

    #[test]
    fn test_format_service_name() {
        assert_eq!(format_service_name("Meet"), "Google Meet");
        assert_eq!(format_service_name("Zoom"), "Zoom");
        assert_eq!(format_service_name("Teams"), "Microsoft Teams");
        assert_eq!(format_service_name("Webex"), "Cisco Webex");
        assert_eq!(format_service_name("CustomService"), "CustomService");
    }

    #[test]
    fn test_build_toast_xml() {
        let xml_with_url = build_toast_xml(
            "週次定例 <A&B>",
            "Meet",
            Some("https://meet.google.com/xyz?authuser=0&hl=ja"),
            1,
        );
        assert!(xml_with_url.contains("週次定例 &lt;A&amp;B&gt;"));
        assert!(xml_with_url.contains("Google Meet ・ 1分後に開始します"));
        assert!(xml_with_url.contains(r#"<action content="参加する" activationType="protocol" arguments="https://meet.google.com/xyz?authuser=0&amp;hl=ja" />"#));
        assert!(xml_with_url.contains(r#"scenario="reminder""#));

        let xml_without_url = build_toast_xml(
            "対面ミーティング",
            "会議",
            None,
            0,
        );
        assert!(xml_without_url.contains("対面ミーティング"));
        assert!(xml_without_url.contains("会議 ・ まもなく始まります"));
        assert!(!xml_without_url.contains("<action content="));
    }
}
