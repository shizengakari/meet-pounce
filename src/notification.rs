use crate::config::{reg_set_dword, reg_set_sz, to_wide};
use std::borrow::Cow;
use std::sync::Once;
use windows::{
    core::{HSTRING, Result},
    Data::Xml::Dom::XmlDocument,
    UI::Notifications::{ToastNotification, ToastNotificationManager},
};

const APP_ID: &str = "MeetPounce";
const APP_DISPLAY_NAME: &str = "Meet Pounce";
static INIT_REGISTER: Once = Once::new();

#[link(name = "winmm")]
extern "system" {
    fn PlaySoundW(pszSound: *const u16, hmod: usize, fdwSound: u32) -> i32;
}

const SND_SYNC: u32 = 0x0000;
const SND_MEMORY: u32 = 0x0004;
const SND_FILENAME: u32 = 0x0002_0000;

/// Windowsに通知アプリ情報を登録する
pub fn ensure_app_registered() {
    INIT_REGISTER.call_once(|| {
        let app_key = format!(r"Software\Classes\AppUserModelId\{APP_ID}");
        let notif_key = format!(r"Software\Microsoft\Windows\CurrentVersion\Notifications\Settings\{APP_ID}");
        reg_set_sz(&app_key, "DisplayName", APP_DISPLAY_NAME);
        reg_set_dword(&notif_key, "ShowInActionCenter", 1);
        reg_set_dword(&notif_key, "Enabled", 1);
    });
}

/// 通知サウンドのファイル名を返す
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
pub fn format_time_text(minutes_left: i64) -> Cow<'static, str> {
    if minutes_left <= 0 {
        Cow::Borrowed("まもなく開始")
    } else {
        Cow::Owned(format!("{minutes_left}分後に開始"))
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
    let title_escaped = xml_escape(title);
    let service_escaped = xml_escape(format_service_name(service));

    let action_xml = meeting_url
        .map(|u| format!(r#"<action content="参加する" activationType="protocol" arguments="{}" />"#, xml_escape(u)))
        .unwrap_or_default();

    format!(
        r#"<toast scenario="reminder">
            <visual>
                <binding template="ToastGeneric">
                    <text>{title_escaped}</text>
                    <text>{service_escaped} ・ {time_text}</text>
                </binding>
            </visual>
            <actions>{action_xml}</actions>
            <audio silent="true" />
        </toast>"#
    )
}

/// WAVデータの音量をインプレースで調整する
pub fn boost_wav_in_place(data: &mut [u8], volume_percent: u32) {
    if data.len() < 44 || &data[0..4] != b"RIFF" || &data[8..12] != b"WAVE" {
        return;
    }

    let mut idx = 12;
    let mut data_range = None;
    while idx + 8 <= data.len() {
        let size = u32::from_le_bytes(data[idx + 4..idx + 8].try_into().unwrap()) as usize;
        if &data[idx..idx + 4] == b"data" {
            let start = idx + 8;
            data_range = Some((start, (start + size).min(data.len())));
            break;
        }
        idx += 8 + size;
    }

    let Some((start, end)) = data_range.filter(|&(s, _)| s < data.len()) else {
        return;
    };

    let max_peak = data[start..end]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&b| i16::from_le_bytes(b).saturating_abs())
        .max()
        .unwrap_or(0);

    if max_peak == 0 {
        return;
    }

    let norm_gain = (31000.0 / (max_peak as f32)).max(1.0);
    let vol = volume_percent.min(500);
    let total_gain = if vol <= 100 {
        (vol as f32) / 100.0
    } else if vol <= 200 {
        let t = (vol - 100) as f32 / 100.0;
        1.0 + (norm_gain - 1.0) * t
    } else {
        let t = (vol - 200) as f32 / 100.0;
        norm_gain * (1.0 + 0.5 * t)
    };

    for chunk in data[start..end].as_chunks_mut::<2>().0 {
        let sample = i16::from_le_bytes(*chunk);
        let mut val = (sample as f32) * total_gain;

        if total_gain > norm_gain {
            let x = val / 32768.0;
            let abs = x.abs();
            if abs > 0.85 {
                let excess = abs - 0.85;
                val = x.signum() * (0.85 + 0.14 * (excess / (1.0 + excess))) * 32768.0;
            }
        }

        *chunk = (val.clamp(-31000.0, 31000.0) as i16).to_le_bytes();
    }
}

/// Windowsの標準WAVファイルを指定音量で再生し、再生完了後にメモリをOSへ返却する
pub fn play_sound(sound_type: &str, sound_volume: u32) {
    if sound_volume == 0 {
        crate::trim_working_set();
        return;
    }
    let Some(file_name) = resolve_sound_file(sound_type) else {
        crate::trim_working_set();
        return;
    };
    let path = format!(r"C:\Windows\Media\{file_name}");

    std::thread::spawn(move || {
        if sound_volume != 100 {
            if let Ok(mut wav_bytes) = std::fs::read(&path) {
                boost_wav_in_place(&mut wav_bytes, sound_volume);
                unsafe {
                    PlaySoundW(wav_bytes.as_ptr() as *const u16, 0, SND_SYNC | SND_MEMORY);
                }
                drop(wav_bytes);
                crate::trim_working_set();
                return;
            }
        }

        let wide = to_wide(&path);
        unsafe {
            PlaySoundW(wide.as_ptr(), 0, SND_SYNC | SND_FILENAME);
        }
        drop(wide);
        crate::trim_working_set();
    });
}

/// 会議・予定のトースト通知をWindowsネイティブで表示し、COMオブジェクトを即時解放する
pub fn show_meeting_toast(
    title: &str,
    service: &str,
    meeting_url: Option<&str>,
    minutes_left: i64,
    sound_type: &str,
    sound_volume: u32,
) -> Result<()> {
    ensure_app_registered();

    {
        let xml = build_toast_xml(title, service, meeting_url, minutes_left);
        let doc = XmlDocument::new()?;
        doc.LoadXml(&HSTRING::from(xml))?;

        let notification = ToastNotification::CreateToastNotification(&doc)?;
        let notifier = ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(APP_ID))
            .or_else(|_| ToastNotificationManager::CreateToastNotifier())?;

        notifier.Show(&notification)?;
    }

    play_sound(sound_type, sound_volume);
    Ok(())
}

/// XML特殊文字をエスケープする
pub fn xml_escape(s: &str) -> Cow<'_, str> {
    if !s.contains(['&', '<', '>', '"', '\'']) {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len() + 8);
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    Cow::Owned(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_xml_escape() {
        assert_eq!(xml_escape("Tom & Jerry"), "Tom &amp; Jerry");
        assert_eq!(xml_escape("<meeting>"), "&lt;meeting&gt;");
        assert!(matches!(xml_escape("regular text"), Cow::Borrowed(_)));
    }

    #[test]
    fn test_resolve_sound_file() {
        assert_eq!(resolve_sound_file("silent"), None);
        assert_eq!(resolve_sound_file("chimes"), Some("chimes.wav"));
        assert_eq!(resolve_sound_file("calendar"), Some("Windows Notify Calendar.wav"));
    }

    #[test]
    fn test_format_time_and_service() {
        assert_eq!(format_time_text(0), "まもなく開始");
        assert_eq!(format_time_text(5), "5分後に開始");
        assert_eq!(format_service_name("Meet"), "Google Meet");
        assert_eq!(format_service_name("予定"), "予定");
    }

    #[test]
    fn test_build_toast_xml() {
        let xml_with_url = build_toast_xml("定例 <A&B>", "Meet", Some("https://meet.google.com/xyz"), 1);
        assert!(xml_with_url.contains("定例 &lt;A&amp;B&gt;"));
        assert!(xml_with_url.contains("Google Meet ・ 1分後に開始"));
        assert!(xml_with_url.contains(r#"<action content="参加する""#));

        let xml_without_url = build_toast_xml("来客対応", "予定", None, 0);
        assert!(xml_without_url.contains("予定 ・ まもなく開始"));
        assert!(!xml_without_url.contains("<action content="));
    }

    #[test]
    fn test_boost_wav_volume() {
        let mut mock_wav = Vec::new();
        mock_wav.extend_from_slice(b"RIFF");
        mock_wav.extend_from_slice(&36u32.to_le_bytes());
        mock_wav.extend_from_slice(b"WAVEfmt ");
        mock_wav.extend_from_slice(&16u32.to_le_bytes());
        mock_wav.extend_from_slice(&[1, 0, 1, 0, 0x44, 0xAC, 0, 0, 0x88, 0x58, 1, 0, 2, 0, 16, 0]);
        mock_wav.extend_from_slice(b"data");
        mock_wav.extend_from_slice(&4u32.to_le_bytes());
        mock_wav.extend_from_slice(&1000i16.to_le_bytes());
        mock_wav.extend_from_slice(&(-2000i16).to_le_bytes());

        boost_wav_in_place(&mut mock_wav, 200);
        let sample2 = i16::from_le_bytes([mock_wav[46], mock_wav[47]]);
        assert_eq!(sample2.abs(), 31000);
    }
}
