use chrono::{DateTime, Local};
use regex::Regex;
use serde::Deserialize;
use std::error::Error;

#[derive(Debug, Clone)]
pub struct MeetingEvent {
    pub id: String,
    pub title: String,
    pub start_time: DateTime<Local>,
    pub meeting_url: Option<String>,
    pub service_type: String,
}

#[derive(Debug, Deserialize)]
struct CalendarListResponse {
    items: Option<Vec<GoogleCalendarEvent>>,
}

#[derive(Debug, Deserialize)]
struct GoogleCalendarEvent {
    id: String,
    summary: Option<String>,
    description: Option<String>,
    location: Option<String>,
    start: Option<EventDateTime>,
    #[serde(rename = "conferenceData")]
    conference_data: Option<ConferenceData>,
}

#[derive(Debug, Deserialize)]
struct EventDateTime {
    #[serde(rename = "dateTime")]
    date_time: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ConferenceData {
    #[serde(rename = "entryPoints")]
    entry_points: Option<Vec<EntryPoint>>,
}

#[derive(Debug, Deserialize)]
struct EntryPoint {
    #[serde(rename = "entryPointType")]
    entry_point_type: Option<String>,
    uri: Option<String>,
}

/// Google Calendar APIから本日の予定を取得する
pub async fn fetch_today_events(
    client: &reqwest::Client,
    access_token: &str,
) -> Result<Vec<MeetingEvent>, Box<dyn Error + Send + Sync>> {
    let now = Local::now();
    let time_min = now.date_naive().and_hms_opt(0, 0, 0).unwrap().and_local_timezone(Local).unwrap().to_rfc3339();
    let time_max = now.date_naive().and_hms_opt(23, 59, 59).unwrap().and_local_timezone(Local).unwrap().to_rfc3339();

    let url = format!(
        "https://www.googleapis.com/calendar/v3/calendars/primary/events?\
        timeMin={}&\
        timeMax={}&\
        singleEvents=true&\
        orderBy=startTime",
        url::form_urlencoded::byte_serialize(time_min.as_bytes()).collect::<String>(),
        url::form_urlencoded::byte_serialize(time_max.as_bytes()).collect::<String>()
    );

    let res = client
        .get(&url)
        .bearer_auth(access_token)
        .send()
        .await?;

    if !res.status().is_success() {
        return Err(format!("カレンダー取得エラー: {}", res.status()).into());
    }

    let list_response: CalendarListResponse = res.json().await?;
    let mut meetings = Vec::new();

    if let Some(items) = list_response.items {
        for item in items {
            let start_time = match &item.start {
                Some(s) => match parse_time(s) {
                    Some(st) => st,
                    None => continue,
                },
                None => continue,
            };

            let title = item.summary.unwrap_or_else(|| "(無題の予定)".to_string());
            let (meeting_url, service_type) = extract_meeting_url(
                item.description.as_deref(),
                item.location.as_deref(),
                item.conference_data.as_ref(),
            );

            meetings.push(MeetingEvent {
                id: item.id,
                title,
                start_time,
                meeting_url,
                service_type,
            });
        }
    }

    Ok(meetings)
}

fn parse_time(edt: &EventDateTime) -> Option<DateTime<Local>> {
    edt.date_time.as_ref().and_then(|dt_str| {
        DateTime::parse_from_rfc3339(dt_str)
            .ok()
            .map(|dt| dt.with_timezone(&Local))
    })
}

/// 会議URLとサービス種別（Meet/Zoom/Teams/Webex等）を抽出する
fn extract_meeting_url(
    description: Option<&str>,
    location: Option<&str>,
    conference_data: Option<&ConferenceData>,
) -> (Option<String>, String) {
    // 1. 公式 conferenceData (Google Meet等)
    if let Some(conf) = conference_data {
        if let Some(entry_points) = &conf.entry_points {
            for ep in entry_points {
                if ep.entry_point_type.as_deref() == Some("video") {
                    if let Some(uri) = &ep.uri {
                        return (Some(uri.clone()), detect_service(uri));
                    }
                }
            }
        }
    }

    // 2. location（場所）と description（説明欄）から正規表現スキャン
    let mut text_buf = String::new();
    if let Some(loc) = location {
        text_buf.push_str(loc);
        text_buf.push(' ');
    }
    if let Some(desc) = description {
        text_buf.push_str(desc);
    }

    let patterns = [
        (r"https?://meet\.google\.com/[a-z0-9\-]+", "Meet"),
        (r"https?://[a-zA-Z0-9.\-]*zoom\.us/(?:j|my)/[a-zA-Z0-9?=_&%-]+", "Zoom"),
        (r"https?://teams\.(?:microsoft|live)\.com/(?:l/meetup-join|meet)/[a-zA-Z0-9?=_&%/-]+", "Teams"),
        (r"https?://[a-zA-Z0-9.\-]*webex\.com/[a-zA-Z0-9?=_&%/-]+", "Webex"),
    ];

    for (pattern, service) in patterns {
        if let Ok(re) = Regex::new(pattern) {
            if let Some(mat) = re.find(&text_buf) {
                return (Some(mat.as_str().to_string()), service.to_string());
            }
        }
    }

    (None, "MTG".to_string())
}

fn detect_service(url: &str) -> String {
    if url.contains("meet.google.com") {
        "Meet".to_string()
    } else if url.contains("zoom.us") {
        "Zoom".to_string()
    } else if url.contains("teams.") {
        "Teams".to_string()
    } else {
        "Webex".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_urls() {
        // Meet from description
        let (url, s) = extract_meeting_url(Some("参加URL: https://meet.google.com/abc-defg-hij"), None, None);
        assert_eq!(url.as_deref(), Some("https://meet.google.com/abc-defg-hij"));
        assert_eq!(s, "Meet");

        // Zoom from location
        let (url, s) = extract_meeting_url(None, Some("https://zoom.us/j/12345678?pwd=abcdef"), None);
        assert_eq!(url.as_deref(), Some("https://zoom.us/j/12345678?pwd=abcdef"));
        assert_eq!(s, "Zoom");

        // Teams
        let (url, s) = extract_meeting_url(Some("Teams会議: https://teams.microsoft.com/l/meetup-join/123-456"), None, None);
        assert_eq!(url.as_deref(), Some("https://teams.microsoft.com/l/meetup-join/123-456"));
        assert_eq!(s, "Teams");

        // Webex
        let (url, s) = extract_meeting_url(Some("Webex: https://company.webex.com/meet/myroom"), None, None);
        assert_eq!(url.as_deref(), Some("https://company.webex.com/meet/myroom"));
        assert_eq!(s, "Webex");

        // URL なし
        let (url, s) = extract_meeting_url(Some("定例ランチ会議"), Some("会議室A"), None);
        assert_eq!(url, None);
        assert_eq!(s, "MTG");
    }

    #[test]
    fn test_extract_from_conference_data() {
        let conf = ConferenceData {
            entry_points: Some(vec![EntryPoint {
                entry_point_type: Some("video".to_string()),
                uri: Some("https://meet.google.com/xyz-uvwx-rst".to_string()),
            }]),
        };

        // conferenceData が最優先される（description に別 URL があっても）
        let (url, s) = extract_meeting_url(
            Some("別のURL https://zoom.us/j/99999999"),
            None,
            Some(&conf),
        );
        assert_eq!(url.as_deref(), Some("https://meet.google.com/xyz-uvwx-rst"));
        assert_eq!(s, "Meet");
    }

    #[test]
    fn test_detect_service() {
        assert_eq!(detect_service("https://meet.google.com/abc-defg-hij"), "Meet");
        assert_eq!(detect_service("https://us02web.zoom.us/j/1234567890"), "Zoom");
        assert_eq!(detect_service("https://teams.live.com/meet/9876543210"), "Teams");
        assert_eq!(detect_service("https://teams.microsoft.com/l/meetup-join/abc"), "Teams");
        assert_eq!(detect_service("https://example.webex.com/meet/user"), "Webex");
    }
}
