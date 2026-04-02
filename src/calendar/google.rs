use crate::calendar::destination::CalendarDestination;
use crate::calendar::domain::{DesiredHubEvent, NormalizedTiming, ReconcileStats};
use crate::calendar::google_auth::GoogleAccessTokenProvider;
use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, Utc};
use reqwest::{Client, Method};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

const NOX_MANAGED_KEY: &str = "noxManaged";
const NOX_MANAGED_VALUE: &str = "true";
const NOX_FINGERPRINT_KEY: &str = "noxFingerprint";
const NOX_CANONICAL_EVENT_KEY: &str = "noxCanonicalEventKey";
const NOX_SOURCE_ID_WINNER_KEY: &str = "noxSourceIdWinner";
const NOX_HAS_CONFLICT_KEY: &str = "noxHasConflict";

pub struct GoogleCalendarDestination {
    client: Client,
    access_token_provider: Arc<GoogleAccessTokenProvider>,
    calendar_id: String,
}

impl GoogleCalendarDestination {
    pub fn new(
        oauth_credentials_path: Option<String>,
        oauth_token_path: Option<String>,
        access_token: Option<String>,
        calendar_id: String,
        timeout_secs: u64,
    ) -> Result<Self, String> {
        let client = Client::builder()
            .no_proxy()
            .timeout(Duration::from_secs(timeout_secs))
            .build()
            .map_err(|e| format!("Failed to build Google Calendar client: {}", e))?;
        let access_token_provider = Arc::new(GoogleAccessTokenProvider::new(
            oauth_credentials_path,
            oauth_token_path,
            access_token,
            timeout_secs,
        )?);
        Ok(Self {
            client,
            access_token_provider,
            calendar_id,
        })
    }

    async fn reconcile_inner(
        &self,
        desired: &[DesiredHubEvent],
        window_start: DateTime<Utc>,
        window_end: DateTime<Utc>,
    ) -> Result<ReconcileStats, String> {
        let mut stats = ReconcileStats::default();
        let listed = self.list_owned_events(window_start, window_end).await?;
        let (mut canonical_events, mut legacy_events): (Vec<_>, Vec<_>) = listed
            .into_iter()
            .partition(|event| event.canonical_event_key.is_some());
        let mut used_existing_ids = HashSet::<String>::new();

        for event in desired {
            if let Some(existing_index) = canonical_events.iter().position(|e| {
                !used_existing_ids.contains(&e.id)
                    && e.canonical_event_key.as_deref() == Some(event.canonical_event_key.as_str())
            }) {
                let existing_id = canonical_events[existing_index].id.clone();
                let existing_fingerprint = canonical_events[existing_index].fingerprint.clone();
                used_existing_ids.insert(existing_id.clone());

                if existing_fingerprint != event.fingerprint {
                    self.update_event(&existing_id, event).await?;
                    stats.updated += 1;
                }
                continue;
            }

            if let Some(legacy_index) = find_legacy_match(&legacy_events, &used_existing_ids, event)
            {
                let legacy_id = legacy_events[legacy_index].id.clone();
                used_existing_ids.insert(legacy_id.clone());
                self.update_event(&legacy_id, event).await?;
                stats.updated += 1;
                continue;
            }

            self.create_event(event).await?;
            stats.created += 1;
        }

        for event in canonical_events.drain(..) {
            if !used_existing_ids.contains(&event.id) {
                self.delete_event(&event.id).await?;
                stats.deleted += 1;
            }
        }

        for event in legacy_events.drain(..) {
            if !used_existing_ids.contains(&event.id) {
                self.delete_event(&event.id).await?;
                stats.deleted += 1;
            }
        }

        Ok(stats)
    }

    async fn list_owned_events(
        &self,
        window_start: DateTime<Utc>,
        window_end: DateTime<Utc>,
    ) -> Result<Vec<OwnedGoogleEvent>, String> {
        let url = format!(
            "https://www.googleapis.com/calendar/v3/calendars/{}/events",
            urlencoding::encode(&self.calendar_id)
        );

        let response = self
            .request(Method::GET, &url)
            .await?
            .query(&[
                ("timeMin", window_start.to_rfc3339()),
                ("timeMax", window_end.to_rfc3339()),
                ("singleEvents", "true".to_string()),
                (
                    "privateExtendedProperty",
                    format!("{}={}", NOX_MANAGED_KEY, NOX_MANAGED_VALUE),
                ),
            ])
            .send()
            .await
            .map_err(|e| format!("Failed to list destination events: {}", e))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(format!(
                "Google Calendar list failed with HTTP {}: {}",
                status,
                sanitize_google_body(&body)
            ));
        }

        let body: GoogleEventsResponse = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse Google Calendar event list: {}", e))?;

        let mut events = Vec::new();
        for item in body.items {
            let private = item
                .extended_properties
                .as_ref()
                .and_then(|properties| properties.private.clone())
                .unwrap_or_default();
            if private.get(NOX_MANAGED_KEY).map(|v| v.as_str()) != Some(NOX_MANAGED_VALUE) {
                continue;
            }

            let fingerprint = private
                .get(NOX_FINGERPRINT_KEY)
                .cloned()
                .unwrap_or_default();
            let canonical_event_key = private.get(NOX_CANONICAL_EVENT_KEY).cloned();
            let timing = parse_google_timing(item.start, item.end)?;
            let kind = timing_kind(&timing);
            events.push(OwnedGoogleEvent {
                id: item.id,
                summary: item.summary.unwrap_or_default(),
                canonical_event_key,
                fingerprint,
                timing,
                kind,
            });
        }

        Ok(events)
    }

    async fn create_event(&self, event: &DesiredHubEvent) -> Result<(), String> {
        let url = format!(
            "https://www.googleapis.com/calendar/v3/calendars/{}/events",
            urlencoding::encode(&self.calendar_id)
        );
        let payload = event_payload(event);
        let response = self
            .request(Method::POST, &url)
            .await?
            .query(&[("sendUpdates", "all")])
            .json(&payload)
            .send()
            .await
            .map_err(|e| format!("Failed to create hub event: {}", e))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(format!(
                "Google Calendar create failed with HTTP {}: {}",
                status,
                sanitize_google_body(&body)
            ));
        }
        Ok(())
    }

    async fn update_event(&self, event_id: &str, event: &DesiredHubEvent) -> Result<(), String> {
        let url = format!(
            "https://www.googleapis.com/calendar/v3/calendars/{}/events/{}",
            urlencoding::encode(&self.calendar_id),
            urlencoding::encode(event_id)
        );
        let payload = event_payload(event);
        let response = self
            .request(Method::PATCH, &url)
            .await?
            .query(&[("sendUpdates", "all")])
            .json(&payload)
            .send()
            .await
            .map_err(|e| format!("Failed to update hub event: {}", e))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(format!(
                "Google Calendar update failed with HTTP {}: {}",
                status,
                sanitize_google_body(&body)
            ));
        }
        Ok(())
    }

    async fn delete_event(&self, event_id: &str) -> Result<(), String> {
        let url = format!(
            "https://www.googleapis.com/calendar/v3/calendars/{}/events/{}",
            urlencoding::encode(&self.calendar_id),
            urlencoding::encode(event_id)
        );
        let response = self
            .request(Method::DELETE, &url)
            .await?
            .send()
            .await
            .map_err(|e| format!("Failed to delete hub event: {}", e))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(format!(
                "Google Calendar delete failed with HTTP {}: {}",
                status,
                sanitize_google_body(&body)
            ));
        }
        Ok(())
    }

    async fn request(&self, method: Method, url: &str) -> Result<reqwest::RequestBuilder, String> {
        let access_token = self.access_token_provider.access_token().await?;
        Ok(self.client.request(method, url).bearer_auth(access_token))
    }
}

#[async_trait]
impl CalendarDestination for GoogleCalendarDestination {
    async fn reconcile(
        &self,
        desired: &[DesiredHubEvent],
        window_start: DateTime<Utc>,
        window_end: DateTime<Utc>,
    ) -> Result<ReconcileStats, String> {
        self.reconcile_inner(desired, window_start, window_end)
            .await
    }
}

fn event_payload(event: &DesiredHubEvent) -> GoogleEventPayload {
    let (start, end) = match &event.timing {
        NormalizedTiming::Timed { start, end } => (
            GoogleEventDateTime {
                date: None,
                date_time: Some(start.to_rfc3339()),
                time_zone: Some("UTC".to_string()),
            },
            GoogleEventDateTime {
                date: None,
                date_time: Some(end.to_rfc3339()),
                time_zone: Some("UTC".to_string()),
            },
        ),
        NormalizedTiming::AllDay {
            start_date,
            end_date_exclusive,
        } => (
            GoogleEventDateTime {
                date: Some(start_date.to_string()),
                date_time: None,
                time_zone: None,
            },
            GoogleEventDateTime {
                date: Some(end_date_exclusive.to_string()),
                date_time: None,
                time_zone: None,
            },
        ),
    };

    let mut private = HashMap::new();
    private.insert(NOX_MANAGED_KEY.to_string(), NOX_MANAGED_VALUE.to_string());
    private.insert(NOX_FINGERPRINT_KEY.to_string(), event.fingerprint.clone());
    private.insert(
        NOX_CANONICAL_EVENT_KEY.to_string(),
        event.canonical_event_key.clone(),
    );
    private.insert(
        NOX_SOURCE_ID_WINNER_KEY.to_string(),
        event.source_id_winner.clone(),
    );
    private.insert(
        NOX_HAS_CONFLICT_KEY.to_string(),
        if event.has_conflict {
            "true".to_string()
        } else {
            "false".to_string()
        },
    );

    GoogleEventPayload {
        summary: event.summary.clone(),
        visibility: Some("private".to_string()),
        transparency: Some("opaque".to_string()),
        start,
        end,
        attendees: Some(
            event
                .invite_targets
                .iter()
                .map(|email| GoogleEventAttendee {
                    email: Some(email.clone()),
                })
                .collect(),
        ),
        extended_properties: GoogleExtendedPropertiesPayload { private },
    }
}

fn parse_google_timing(
    start: GoogleEventDateTime,
    end: GoogleEventDateTime,
) -> Result<NormalizedTiming, String> {
    match (start.date, end.date, start.date_time, end.date_time) {
        (Some(start_date), Some(end_date), _, _) => {
            let start_date = NaiveDate::parse_from_str(&start_date, "%Y-%m-%d")
                .map_err(|e| format!("Invalid Google all-day start date: {}", e))?;
            let end_date_exclusive = NaiveDate::parse_from_str(&end_date, "%Y-%m-%d")
                .map_err(|e| format!("Invalid Google all-day end date: {}", e))?;
            Ok(NormalizedTiming::AllDay {
                start_date,
                end_date_exclusive,
            })
        }
        (_, _, Some(start_dt), Some(end_dt)) => {
            let start = DateTime::parse_from_rfc3339(&start_dt)
                .map_err(|e| format!("Invalid Google timed start: {}", e))?
                .with_timezone(&Utc);
            let end = DateTime::parse_from_rfc3339(&end_dt)
                .map_err(|e| format!("Invalid Google timed end: {}", e))?
                .with_timezone(&Utc);
            Ok(NormalizedTiming::Timed { start, end })
        }
        _ => Err("Google event missing supported start/end fields".to_string()),
    }
}

fn sanitize_google_body(body: &str) -> String {
    body.chars().take(256).collect()
}

fn find_legacy_match(
    legacy_events: &[OwnedGoogleEvent],
    used_existing_ids: &HashSet<String>,
    desired: &DesiredHubEvent,
) -> Option<usize> {
    let matches = legacy_events
        .iter()
        .enumerate()
        .filter(|(_, event)| {
            !used_existing_ids.contains(&event.id)
                && event.summary == desired.summary
                && event.kind == timing_kind(&desired.timing)
                && event.timing == desired.timing
        })
        .map(|(index, _)| index)
        .collect::<Vec<_>>();

    if matches.len() == 1 {
        matches.into_iter().next()
    } else {
        None
    }
}

fn timing_kind(timing: &NormalizedTiming) -> &'static str {
    match timing {
        NormalizedTiming::Timed { .. } => "timed",
        NormalizedTiming::AllDay { .. } => "all-day",
    }
}

#[derive(Debug)]
struct OwnedGoogleEvent {
    id: String,
    summary: String,
    canonical_event_key: Option<String>,
    fingerprint: String,
    timing: NormalizedTiming,
    kind: &'static str,
}

#[derive(Debug, Deserialize)]
struct GoogleEventsResponse {
    #[serde(default)]
    items: Vec<GoogleEventItem>,
}

#[derive(Debug, Deserialize)]
struct GoogleEventItem {
    id: String,
    summary: Option<String>,
    start: GoogleEventDateTime,
    end: GoogleEventDateTime,
    #[serde(rename = "extendedProperties")]
    extended_properties: Option<GoogleExtendedProperties>,
}

#[derive(Debug, Deserialize)]
struct GoogleExtendedProperties {
    private: Option<HashMap<String, String>>,
}

#[derive(Debug, Serialize)]
struct GoogleEventPayload {
    summary: String,
    visibility: Option<String>,
    transparency: Option<String>,
    start: GoogleEventDateTime,
    end: GoogleEventDateTime,
    #[serde(skip_serializing_if = "Option::is_none")]
    attendees: Option<Vec<GoogleEventAttendee>>,
    #[serde(rename = "extendedProperties")]
    extended_properties: GoogleExtendedPropertiesPayload,
}

#[derive(Debug, Serialize)]
struct GoogleExtendedPropertiesPayload {
    private: HashMap<String, String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct GoogleEventDateTime {
    #[serde(rename = "date")]
    date: Option<String>,
    #[serde(rename = "dateTime")]
    date_time: Option<String>,
    #[serde(rename = "timeZone")]
    time_zone: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct GoogleEventAttendee {
    email: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::{OwnedGoogleEvent, find_legacy_match};
    use crate::calendar::domain::{DesiredHubEvent, NormalizedTiming};
    use chrono::{NaiveDate, TimeZone, Utc};
    use std::collections::HashSet;

    #[test]
    fn legacy_match_requires_unique_exact_summary_and_timing_match() {
        let desired = DesiredHubEvent::new(
            "uid-1".into(),
            "globant".into(),
            NormalizedTiming::Timed {
                start: Utc.with_ymd_and_hms(2026, 3, 20, 15, 0, 0).unwrap(),
                end: Utc.with_ymd_and_hms(2026, 3, 20, 16, 0, 0).unwrap(),
            },
            "Busy - Globant".into(),
            "business".into(),
            vec!["carlos@personal.com".into()],
            vec!["carlos@globant.com".into()],
            false,
        );

        let legacy_events = vec![OwnedGoogleEvent {
            id: "legacy-1".into(),
            summary: "Busy - Globant".into(),
            canonical_event_key: None,
            fingerprint: String::new(),
            timing: NormalizedTiming::Timed {
                start: Utc.with_ymd_and_hms(2026, 3, 20, 15, 0, 0).unwrap(),
                end: Utc.with_ymd_and_hms(2026, 3, 20, 16, 0, 0).unwrap(),
            },
            kind: "timed",
        }];

        assert_eq!(
            find_legacy_match(&legacy_events, &HashSet::new(), &desired),
            Some(0)
        );
    }

    #[test]
    fn legacy_match_rejects_ambiguous_candidates() {
        let desired = DesiredHubEvent::new(
            "uid-1".into(),
            "globant".into(),
            NormalizedTiming::AllDay {
                start_date: NaiveDate::from_ymd_opt(2026, 3, 20).unwrap(),
                end_date_exclusive: NaiveDate::from_ymd_opt(2026, 3, 21).unwrap(),
            },
            "Busy - Globant".into(),
            "business".into(),
            vec!["carlos@personal.com".into()],
            vec!["carlos@globant.com".into()],
            false,
        );

        let legacy_events = vec![
            OwnedGoogleEvent {
                id: "legacy-1".into(),
                summary: "Busy - Globant".into(),
                canonical_event_key: None,
                fingerprint: String::new(),
                timing: NormalizedTiming::AllDay {
                    start_date: NaiveDate::from_ymd_opt(2026, 3, 20).unwrap(),
                    end_date_exclusive: NaiveDate::from_ymd_opt(2026, 3, 21).unwrap(),
                },
                kind: "all-day",
            },
            OwnedGoogleEvent {
                id: "legacy-2".into(),
                summary: "Busy - Globant".into(),
                canonical_event_key: None,
                fingerprint: String::new(),
                timing: NormalizedTiming::AllDay {
                    start_date: NaiveDate::from_ymd_opt(2026, 3, 20).unwrap(),
                    end_date_exclusive: NaiveDate::from_ymd_opt(2026, 3, 21).unwrap(),
                },
                kind: "all-day",
            },
        ];

        assert_eq!(
            find_legacy_match(&legacy_events, &HashSet::new(), &desired),
            None
        );
    }
}
