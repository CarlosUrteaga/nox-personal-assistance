use crate::calendar::destination::CalendarDestination;
use crate::calendar::domain::{NormalizedTiming, ReconcileStats, ResolvedBlocker};
use async_trait::async_trait;
use chrono::{DateTime, NaiveDate, Utc};
use reqwest::{Client, Method};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::time::Duration;

const NOX_MANAGED_KEY: &str = "noxManaged";
const NOX_MANAGED_VALUE: &str = "true";
const NOX_FINGERPRINT_KEY: &str = "noxFingerprint";
const NOX_SOURCE_ID_KEY: &str = "noxSourceId";
const NOX_CATEGORY_KEY: &str = "noxCategory";

pub struct GoogleCalendarDestination {
    client: Client,
    access_token: String,
    calendar_id: String,
}

impl GoogleCalendarDestination {
    pub fn new(access_token: String, calendar_id: String, timeout_secs: u64) -> Result<Self, String> {
        let client = Client::builder()
            .timeout(Duration::from_secs(timeout_secs))
            .build()
            .map_err(|e| format!("Failed to build Google Calendar client: {}", e))?;
        Ok(Self {
            client,
            access_token,
            calendar_id,
        })
    }

    async fn reconcile_inner(
        &self,
        desired: &[ResolvedBlocker],
        window_start: DateTime<Utc>,
        window_end: DateTime<Utc>,
    ) -> Result<ReconcileStats, String> {
        let mut stats = ReconcileStats::default();
        let mut existing = self.list_owned_events(window_start, window_end).await?;
        let mut used_existing_ids = HashSet::new();

        for blocker in desired {
            if let Some(existing_index) = existing
                .iter()
                .position(|event| event.fingerprint == blocker.fingerprint)
            {
                let existing_event = &existing[existing_index];
                used_existing_ids.insert(existing_event.id.clone());
                if existing_event.timing != blocker.timing
                    || existing_event.summary != blocker.label
                    || !same_attendees(&existing_event.attendees, &blocker.attendees)
                {
                    self.update_event(&existing_event.id, blocker).await?;
                    stats.updated += 1;
                }
                continue;
            }

            if let Some(existing_index) = existing.iter().position(|event| {
                !used_existing_ids.contains(&event.id)
                    && event.source_id == blocker.source_id
                    && event.summary == blocker.label
                    && event.timing.same_kind(&blocker.timing)
                    && event.timing.overlaps(&blocker.timing)
            }) {
                let existing_id = existing[existing_index].id.clone();
                used_existing_ids.insert(existing_id.clone());
                self.update_event(&existing_id, blocker).await?;
                stats.updated += 1;
                continue;
            }

            self.create_event(blocker).await?;
            stats.created += 1;
        }

        for event in existing.drain(..) {
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
            .map_err(|e| format!("Failed to list destination blockers: {}", e))?;

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

            let Some(fingerprint) = private.get(NOX_FINGERPRINT_KEY).cloned() else {
                continue;
            };
            let Some(source_id) = private.get(NOX_SOURCE_ID_KEY).cloned() else {
                continue;
            };
            let timing = parse_google_timing(item.start, item.end)?;
            events.push(OwnedGoogleEvent {
                id: item.id,
                summary: item.summary.unwrap_or_default(),
                source_id,
                fingerprint,
                attendees: item
                    .attendees
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|attendee| attendee.email)
                    .collect(),
                timing,
            });
        }

        Ok(events)
    }

    async fn create_event(&self, blocker: &ResolvedBlocker) -> Result<(), String> {
        let url = format!(
            "https://www.googleapis.com/calendar/v3/calendars/{}/events",
            urlencoding::encode(&self.calendar_id)
        );
        let payload = event_payload(blocker);
        let response = self
            .request(Method::POST, &url)
            .query(&[("sendUpdates", "all")])
            .json(&payload)
            .send()
            .await
            .map_err(|e| format!("Failed to create blocker: {}", e))?;

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

    async fn update_event(&self, event_id: &str, blocker: &ResolvedBlocker) -> Result<(), String> {
        let url = format!(
            "https://www.googleapis.com/calendar/v3/calendars/{}/events/{}",
            urlencoding::encode(&self.calendar_id),
            urlencoding::encode(event_id)
        );
        let payload = event_payload(blocker);
        let response = self
            .request(Method::PATCH, &url)
            .query(&[("sendUpdates", "all")])
            .json(&payload)
            .send()
            .await
            .map_err(|e| format!("Failed to update blocker: {}", e))?;

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
            .send()
            .await
            .map_err(|e| format!("Failed to delete blocker: {}", e))?;

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

    fn request(&self, method: Method, url: &str) -> reqwest::RequestBuilder {
        self.client
            .request(method, url)
            .bearer_auth(&self.access_token)
    }
}

#[async_trait]
impl CalendarDestination for GoogleCalendarDestination {
    async fn reconcile(
        &self,
        desired: &[ResolvedBlocker],
        window_start: DateTime<Utc>,
        window_end: DateTime<Utc>,
    ) -> Result<ReconcileStats, String> {
        self.reconcile_inner(desired, window_start, window_end).await
    }
}

fn event_payload(blocker: &ResolvedBlocker) -> GoogleEventPayload {
    let (start, end) = match &blocker.timing {
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
    private.insert(NOX_FINGERPRINT_KEY.to_string(), blocker.fingerprint.clone());
    private.insert(NOX_SOURCE_ID_KEY.to_string(), blocker.source_id.clone());
    private.insert(NOX_CATEGORY_KEY.to_string(), blocker.category.clone());

    GoogleEventPayload {
        summary: blocker.label.clone(),
        visibility: Some("private".to_string()),
        transparency: Some("opaque".to_string()),
        start,
        end,
        attendees: blocker
            .attendees
            .iter()
            .map(|email| GoogleEventAttendee {
                email: Some(email.clone()),
            })
            .collect(),
        extended_properties: GoogleExtendedPropertiesPayload {
            private,
        },
    }
}

fn same_attendees(left: &[String], right: &[String]) -> bool {
    let mut left = left.iter().map(|value| value.trim().to_ascii_lowercase()).collect::<Vec<_>>();
    let mut right = right.iter().map(|value| value.trim().to_ascii_lowercase()).collect::<Vec<_>>();
    left.sort();
    right.sort();
    left.dedup();
    right.dedup();
    left == right
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

#[derive(Debug)]
struct OwnedGoogleEvent {
    id: String,
    summary: String,
    source_id: String,
    fingerprint: String,
    attendees: Vec<String>,
    timing: NormalizedTiming,
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
    attendees: Option<Vec<GoogleEventAttendee>>,
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
    attendees: Vec<GoogleEventAttendee>,
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
