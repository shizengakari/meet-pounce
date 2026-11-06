use chrono::{DateTime, Duration, Local, NaiveTime};
use serde::Deserialize;
use std::error::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeetingEvent {
    pub id: String,
    pub title: String,
    pub start_time: DateTime<Local>,
    pub meeting_url: Option<String>,
    pub service_type: &'static str,
}

pub enum FetchResult {
    Unchanged,
    Updated(Vec<MeetingEvent>, Option<String>),
}

#[derive(Debug, Deserialize)]
struct CalendarListResponse {
    etag: Option<String>,
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

/// Google Calendar APIから本日の今後の予定を差分取得する
pub async fn fetch_today_events(
    client: &reqwest::Client,
    access_token: &str,
    prev_etag: Option<&str>,
) -> Result<FetchResult, Box<dyn Error + Send + Sync>> {
    let now = Local::now();
    let today = now.date_naive();
    let time_min = today
        .and_time(NaiveTime::from_hms_opt(0, 0, 0).unwrap())
        .and_local_timezone(Local)
        .unwrap()
        .to_rfc3339();
    let time_max = today
        .and_time(NaiveTime::from_hms_opt(23, 59, 59).unwrap())
        .and_local_timezone(Local)
        .unwrap()
        .to_rfc3339();

    let mut req = client
        .get("https://www.googleapis.com/calendar/v3/calendars/primary/events")
        .bearer_auth(access_token)
        .query(&[
            ("timeMin", time_min.as_str()),
            ("timeMax", time_max.as_str()),
            ("singleEvents", "true"),
            ("orderBy", "startTime"),
            (
                "fields",
                "etag,items(id,summary,description,location,start/dateTime,conferenceData/entryPoints(entryPointType,uri))",
            ),
        ]);

    if let Some(etag) = prev_etag {
        req = req.header("If-None-Match", etag);
    }

    let res = req.send().await?;

    if res.status().as_u16() == 304 {
        return Ok(FetchResult::Unchanged);
    }

    if !res.status().is_success() {
        return Err(format!("カレンダー取得エラー: {}", res.status()).into());
    }

    let header_etag = res
        .headers()
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .map(String::from);

    let list_response: CalendarListResponse = res.json().await?;
    let new_etag = header_etag.or(list_response.etag);
    let cutoff = now - Duration::minutes(2);

    let meetings = list_response
        .items
        .unwrap_or_default()
        .into_iter()
        .filter_map(|item| {
            let start_time = parse_time(item.start.as_ref()?)?;
            if start_time <= cutoff {
                return None;
            }
            let title = item.summary.unwrap_or_else(|| "無題の予定".to_string());
            let (meeting_url, service_type) = extract_meeting_url(
                item.description.as_deref(),
                item.location.as_deref(),
                item.conference_data.as_ref(),
            );

            Some(MeetingEvent {
                id: item.id,
                title,
                start_time,
                meeting_url,
                service_type,
            })
        })
        .collect();

    Ok(FetchResult::Updated(meetings, new_etag))
}

fn parse_time(edt: &EventDateTime) -> Option<DateTime<Local>> {
    edt.date_time
        .as_deref()
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(Into::into)
}

/// 会議URLとサービス種別を抽出する
fn extract_meeting_url(
    description: Option<&str>,
    location: Option<&str>,
    conference_data: Option<&ConferenceData>,
) -> (Option<String>, &'static str) {
    if let Some(uri) = conference_data
        .and_then(|c| c.entry_points.as_ref())
        .and_then(|eps| eps.iter().find(|e| e.entry_point_type.as_deref() == Some("video")))
        .and_then(|e| e.uri.as_deref())
    {
        return (Some(uri.to_string()), detect_service(uri));
    }

    if let Some((url, service)) = [location, description].into_iter().flatten().find_map(scan_service_url) {
        return (Some(url), service);
    }

    (None, "予定")
}

fn is_url_char(b: u8, allow_slash: bool) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b'?' | b'=' | b'_' | b'&' | b'%' | b'-') || (allow_slash && b == b'/')
}

fn scan_service_url(text: &str) -> Option<(String, &'static str)> {
    for (idx, _) in text.match_indices("http") {
        let rest = &text[idx..];
        let Some(body) = rest.strip_prefix("https://").or_else(|| rest.strip_prefix("http://")) else {
            continue;
        };
        let Some((host, path)) = body.split_once('/') else {
            continue;
        };

        let (valid_len, service) = if host == "meet.google.com" {
            let len = path.bytes().take_while(|&b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-').count();
            (len, "Meet")
        } else if (host == "zoom.us" || host.ends_with(".zoom.us"))
            && host.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
        {
            let pfx = if path.starts_with("j/") { 2 } else if path.starts_with("my/") { 3 } else { 0 };
            let len = if pfx > 0 { pfx + path[pfx..].bytes().take_while(|&b| is_url_char(b, false)).count() } else { 0 };
            if len > pfx { (len, "Zoom") } else { (0, "") }
        } else if host == "teams.microsoft.com" || host == "teams.live.com" {
            let pfx = if path.starts_with("l/meetup-join/") { 14 } else if path.starts_with("meet/") { 5 } else { 0 };
            let len = if pfx > 0 { pfx + path[pfx..].bytes().take_while(|&b| is_url_char(b, true)).count() } else { 0 };
            if len > pfx { (len, "Teams") } else { (0, "") }
        } else if (host == "webex.com" || host.ends_with(".webex.com"))
            && host.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
        {
            let len = path.bytes().take_while(|&b| is_url_char(b, true)).count();
            (len, "Webex")
        } else {
            (0, "")
        };

        if valid_len > 0 {
            let scheme_len = rest.len() - body.len();
            let total = scheme_len + host.len() + 1 + valid_len;
            return Some((rest[..total].to_string(), service));
        }
    }
    None
}

fn detect_service(url: &str) -> &'static str {
    if url.contains("meet.google.com") {
        "Meet"
    } else if url.contains("zoom.us") {
        "Zoom"
    } else if url.contains("teams.") {
        "Teams"
    } else {
        "Webex"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_urls() {
        let (url, s) = extract_meeting_url(Some("参加URL: https://meet.google.com/abc-defg-hij"), None, None);
        assert_eq!(url.as_deref(), Some("https://meet.google.com/abc-defg-hij"));
        assert_eq!(s, "Meet");

        let (url, s) = extract_meeting_url(None, Some("https://zoom.us/j/12345678?pwd=abcdef"), None);
        assert_eq!(url.as_deref(), Some("https://zoom.us/j/12345678?pwd=abcdef"));
        assert_eq!(s, "Zoom");

        let (url, s) = extract_meeting_url(Some("Teams会議: https://teams.microsoft.com/l/meetup-join/123-456"), None, None);
        assert_eq!(url.as_deref(), Some("https://teams.microsoft.com/l/meetup-join/123-456"));
        assert_eq!(s, "Teams");

        let (url, s) = extract_meeting_url(Some("Webex: https://company.webex.com/meet/myroom"), None, None);
        assert_eq!(url.as_deref(), Some("https://company.webex.com/meet/myroom"));
        assert_eq!(s, "Webex");

        let (url, s) = extract_meeting_url(Some("定例ランチ会議"), Some("会議室A"), None);
        assert_eq!(url, None);
        assert_eq!(s, "予定");
    }

    #[test]
    fn test_extract_from_conference_data() {
        let conf = ConferenceData {
            entry_points: Some(vec![EntryPoint {
                entry_point_type: Some("video".to_string()),
                uri: Some("https://meet.google.com/xyz-uvwx-rst".to_string()),
            }]),
        };

        let (url, s) = extract_meeting_url(Some("別のURL https://zoom.us/j/99999999"), None, Some(&conf));
        assert_eq!(url.as_deref(), Some("https://meet.google.com/xyz-uvwx-rst"));
        assert_eq!(s, "Meet");
    }

    #[test]
    fn test_detect_service() {
        assert_eq!(detect_service("https://meet.google.com/abc-defg-hij"), "Meet");
        assert_eq!(detect_service("https://us02web.zoom.us/j/1234567890"), "Zoom");
        assert_eq!(detect_service("https://teams.live.com/meet/9876543210"), "Teams");
        assert_eq!(detect_service("https://example.webex.com/meet/user"), "Webex");
    }
}
