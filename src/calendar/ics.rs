use crate::calendar::domain::{NormalizedEvent, NormalizedTiming};
use crate::config::CalendarSourceConfig;
use chrono::{
    DateTime, Datelike, Days, Duration, NaiveDate, NaiveDateTime, TimeZone, Utc, Weekday,
};
use chrono_tz::Tz;
use ical::parser::ical::IcalParser;
use ical::parser::ical::component::IcalEvent;
use reqwest::Client;
use std::collections::{HashMap, HashSet};
use std::io::Cursor;
use std::str::FromStr;
use std::time::Duration as StdDuration;

pub struct IcsFetcher {
    client: Client,
}

impl IcsFetcher {
    pub fn new(timeout_secs: u64) -> Result<Self, String> {
        let client = Client::builder()
            .timeout(StdDuration::from_secs(timeout_secs))
            .build()
            .map_err(|e| format!("Failed to build ICS HTTP client: {}", e))?;
        Ok(Self { client })
    }

    pub async fn fetch_source_events(
        &self,
        source: &CalendarSourceConfig,
        window_start: DateTime<Utc>,
        window_end: DateTime<Utc>,
    ) -> Result<Vec<NormalizedEvent>, String> {
        log::info!("Fetching ICS calendar source: source_id={}", source.id);
        let body = self
            .client
            .get(&source.url)
            .send()
            .await
            .map_err(|e| format!("Failed to fetch ICS source '{}': {}", source.id, e))?
            .error_for_status()
            .map_err(|e| format!("ICS source '{}' returned error: {}", source.id, e))?
            .text()
            .await
            .map_err(|e| format!("Failed to read ICS source '{}': {}", source.id, e))?;

        let reader = Cursor::new(body.into_bytes());
        let mut parsed = IcalParser::new(reader);
        let mut normalized = Vec::new();

        while let Some(calendar) = parsed.next() {
            let calendar = calendar
                .map_err(|e| format!("Failed to parse ICS source '{}': {}", source.id, e))?;
            for event in calendar.events {
                normalized.extend(expand_event(source, &event, window_start, window_end)?);
            }
        }

        log::info!(
            "Fetched ICS events: source_id={}, normalized_events={}",
            source.id,
            normalized.len()
        );

        Ok(normalized)
    }
}

fn expand_event(
    source: &CalendarSourceConfig,
    event: &IcalEvent,
    window_start: DateTime<Utc>,
    window_end: DateTime<Utc>,
) -> Result<Vec<NormalizedEvent>, String> {
    let Some(dtstart_property) = property(event, "DTSTART") else {
        return Ok(Vec::new());
    };

    let recurrence_id = property_value(event, "RECURRENCE-ID");
    let rrule = property_value(event, "RRULE");
    let exdates = parse_exdates(event, dtstart_property)?;
    let start = parse_property_time(dtstart_property)?;
    let end = match property(event, "DTEND") {
        Some(property) => parse_property_time(property)?,
        None => default_end_from_start(&start)?,
    };

    let base_timing = match (start, end) {
        (ParsedTime::Timed(start), ParsedTime::Timed(end)) if end > start => {
            NormalizedTiming::Timed { start, end }
        }
        (ParsedTime::AllDay(start_date), ParsedTime::AllDay(end_date_exclusive))
            if end_date_exclusive > start_date =>
        {
            NormalizedTiming::AllDay {
                start_date,
                end_date_exclusive,
            }
        }
        (ParsedTime::AllDay(start_date), ParsedTime::Timed(_)) => NormalizedTiming::AllDay {
            start_date,
            end_date_exclusive: start_date
                .checked_add_days(Days::new(1))
                .ok_or_else(|| format!("Invalid all-day end for source '{}'", source.id))?,
        },
        (ParsedTime::Timed(start), ParsedTime::AllDay(end_date_exclusive)) => {
            let end = end_date_exclusive
                .and_hms_opt(0, 0, 0)
                .expect("valid all-day end")
                .and_utc();
            if end <= start {
                return Ok(Vec::new());
            }
            NormalizedTiming::Timed { start, end }
        }
        _ => return Ok(Vec::new()),
    };

    let candidates = if recurrence_id.is_some() || rrule.is_none() {
        vec![base_timing]
    } else {
        expand_rrule(
            &base_timing,
            rrule.as_deref(),
            &exdates,
            window_start,
            window_end,
        )?
    };

    Ok(candidates
        .into_iter()
        .filter(|timing| timing.intersects_window(window_start, window_end))
        .map(|timing| {
            NormalizedEvent::new(
                source.id.clone(),
                source.label.clone(),
                source.priority,
                source.category.clone(),
                source.owner_email.clone(),
                timing,
            )
        })
        .collect())
}

fn expand_rrule(
    base_timing: &NormalizedTiming,
    rrule: Option<&str>,
    exdates: &HashSet<String>,
    window_start: DateTime<Utc>,
    window_end: DateTime<Utc>,
) -> Result<Vec<NormalizedTiming>, String> {
    let Some(rrule) = rrule else {
        return Ok(vec![base_timing.clone()]);
    };

    let parsed_rule = ParsedRRule::parse(rrule)?;
    match base_timing {
        NormalizedTiming::Timed { start, end } => expand_timed_rrule(
            *start,
            *end - *start,
            &parsed_rule,
            exdates,
            window_start,
            window_end,
        ),
        NormalizedTiming::AllDay {
            start_date,
            end_date_exclusive,
        } => {
            let span_days = end_date_exclusive
                .signed_duration_since(*start_date)
                .num_days()
                .max(1);
            expand_all_day_rrule(
                *start_date,
                span_days,
                &parsed_rule,
                exdates,
                window_start,
                window_end,
            )
        }
    }
}

fn expand_timed_rrule(
    base_start: DateTime<Utc>,
    duration: Duration,
    rule: &ParsedRRule,
    exdates: &HashSet<String>,
    window_start: DateTime<Utc>,
    window_end: DateTime<Utc>,
) -> Result<Vec<NormalizedTiming>, String> {
    let mut results = Vec::new();
    match rule.freq.as_str() {
        "DAILY" => {
            let mut current = base_start;
            let mut seen = 0usize;
            loop {
                if let Some(count) = rule.count {
                    if seen >= count {
                        break;
                    }
                }
                if is_until_exceeded(current, rule.until) {
                    break;
                }
                if current + duration > window_start
                    && current < window_end
                    && !exdates.contains(&current.to_rfc3339())
                {
                    results.push(NormalizedTiming::Timed {
                        start: current,
                        end: current + duration,
                    });
                }
                current += Duration::days(i64::from(rule.interval));
                seen += 1;
                if current > window_end + Duration::days(i64::from(rule.interval)) {
                    break;
                }
            }
        }
        "WEEKLY" => {
            let byday = if rule.byday.is_empty() {
                vec![base_start.weekday()]
            } else {
                rule.byday.clone()
            };
            let week_start = base_start.date_naive();
            let mut day = week_start;
            let mut seen = 0usize;
            while let Some(next_day) = day.checked_add_days(Days::new(1)) {
                let weeks_since = day
                    .signed_duration_since(week_start)
                    .num_days()
                    .div_euclid(7);
                if weeks_since >= 0
                    && weeks_since % i64::from(rule.interval) == 0
                    && byday.contains(&day.weekday())
                {
                    let current = day.and_time(base_start.time()).and_utc();
                    if current >= base_start {
                        if let Some(count) = rule.count {
                            if seen >= count {
                                break;
                            }
                        }
                        if is_until_exceeded(current, rule.until) {
                            break;
                        }
                        if current + duration > window_start
                            && current < window_end
                            && !exdates.contains(&current.to_rfc3339())
                        {
                            results.push(NormalizedTiming::Timed {
                                start: current,
                                end: current + duration,
                            });
                        }
                        seen += 1;
                    }
                }
                if day.and_hms_opt(0, 0, 0).expect("valid week day").and_utc()
                    > window_end + Duration::weeks(i64::from(rule.interval))
                {
                    break;
                }
                day = next_day;
            }
        }
        other => {
            return Err(format!("Unsupported RRULE frequency '{}'", other));
        }
    }

    if results.is_empty() && base_start < window_end && base_start + duration > window_start {
        results.push(NormalizedTiming::Timed {
            start: base_start,
            end: base_start + duration,
        });
    }

    Ok(results)
}

fn expand_all_day_rrule(
    base_start: NaiveDate,
    span_days: i64,
    rule: &ParsedRRule,
    exdates: &HashSet<String>,
    window_start: DateTime<Utc>,
    window_end: DateTime<Utc>,
) -> Result<Vec<NormalizedTiming>, String> {
    let mut results = Vec::new();
    match rule.freq.as_str() {
        "DAILY" => {
            let mut current = base_start;
            let mut seen = 0usize;
            loop {
                if let Some(count) = rule.count {
                    if seen >= count {
                        break;
                    }
                }
                let current_dt = current.and_hms_opt(0, 0, 0).expect("valid date").and_utc();
                if is_until_exceeded(current_dt, rule.until) {
                    break;
                }
                let end = current
                    .checked_add_days(Days::new(span_days as u64))
                    .ok_or_else(|| "Failed to expand all-day recurring event".to_string())?;
                if current_dt < window_end
                    && end.and_hms_opt(0, 0, 0).expect("valid end").and_utc() > window_start
                    && !exdates.contains(&current.to_string())
                {
                    results.push(NormalizedTiming::AllDay {
                        start_date: current,
                        end_date_exclusive: end,
                    });
                }
                current = current
                    .checked_add_days(Days::new(u64::from(rule.interval)))
                    .ok_or_else(|| "Failed to advance all-day recurring event".to_string())?;
                seen += 1;
                if current.and_hms_opt(0, 0, 0).expect("valid next").and_utc()
                    > window_end + Duration::days(i64::from(rule.interval))
                {
                    break;
                }
            }
        }
        "WEEKLY" => {
            let byday = if rule.byday.is_empty() {
                vec![base_start.weekday()]
            } else {
                rule.byday.clone()
            };
            let mut day = base_start;
            let mut seen = 0usize;
            while let Some(next_day) = day.checked_add_days(Days::new(1)) {
                let weeks_since = day
                    .signed_duration_since(base_start)
                    .num_days()
                    .div_euclid(7);
                if weeks_since >= 0
                    && weeks_since % i64::from(rule.interval) == 0
                    && byday.contains(&day.weekday())
                {
                    if let Some(count) = rule.count {
                        if seen >= count {
                            break;
                        }
                    }
                    let current_dt = day.and_hms_opt(0, 0, 0).expect("valid day").and_utc();
                    if is_until_exceeded(current_dt, rule.until) {
                        break;
                    }
                    let end = day
                        .checked_add_days(Days::new(span_days as u64))
                        .ok_or_else(|| {
                            "Failed to expand weekly all-day recurring event".to_string()
                        })?;
                    if current_dt < window_end
                        && end.and_hms_opt(0, 0, 0).expect("valid end").and_utc() > window_start
                        && !exdates.contains(&day.to_string())
                    {
                        results.push(NormalizedTiming::AllDay {
                            start_date: day,
                            end_date_exclusive: end,
                        });
                    }
                    seen += 1;
                }
                if day.and_hms_opt(0, 0, 0).expect("valid check").and_utc()
                    > window_end + Duration::weeks(i64::from(rule.interval))
                {
                    break;
                }
                day = next_day;
            }
        }
        other => {
            return Err(format!("Unsupported RRULE frequency '{}'", other));
        }
    }

    Ok(results)
}

fn property<'a>(event: &'a IcalEvent, name: &str) -> Option<&'a ical::property::Property> {
    event
        .properties
        .iter()
        .find(|property| property.name.eq_ignore_ascii_case(name))
}

fn property_value(event: &IcalEvent, name: &str) -> Option<String> {
    property(event, name).and_then(|property| property.value.clone())
}

fn parse_exdates(
    event: &IcalEvent,
    dtstart_property: &ical::property::Property,
) -> Result<HashSet<String>, String> {
    let mut values = HashSet::new();
    for property in event
        .properties
        .iter()
        .filter(|property| property.name.eq_ignore_ascii_case("EXDATE"))
    {
        for raw in property.value.clone().unwrap_or_default().split(',') {
            match parse_property_time_with_override(dtstart_property, raw)? {
                ParsedTime::Timed(dt) => {
                    values.insert(dt.to_rfc3339());
                }
                ParsedTime::AllDay(date) => {
                    values.insert(date.to_string());
                }
            }
        }
    }
    Ok(values)
}

fn default_end_from_start(start: &ParsedTime) -> Result<ParsedTime, String> {
    match start {
        ParsedTime::Timed(start) => Ok(ParsedTime::Timed(*start + Duration::minutes(30))),
        ParsedTime::AllDay(date) => Ok(ParsedTime::AllDay(
            date.checked_add_days(Days::new(1))
                .ok_or_else(|| "Invalid all-day default end".to_string())?,
        )),
    }
}

fn parse_property_time(property: &ical::property::Property) -> Result<ParsedTime, String> {
    parse_property_time_with_override(property, property.value.as_deref().unwrap_or_default())
}

fn parse_property_time_with_override(
    property: &ical::property::Property,
    raw: &str,
) -> Result<ParsedTime, String> {
    let value_type = property_param(property, "VALUE");
    let tzid = property_param(property, "TZID");

    if value_type.as_deref() == Some("DATE") || raw.len() == 8 {
        let date = NaiveDate::parse_from_str(raw, "%Y%m%d")
            .map_err(|e| format!("Invalid ICS all-day date '{}': {}", raw, e))?;
        return Ok(ParsedTime::AllDay(date));
    }

    if raw.ends_with('Z') {
        let trimmed = raw.trim_end_matches('Z');
        let parsed = NaiveDateTime::parse_from_str(trimmed, "%Y%m%dT%H%M%S")
            .map_err(|e| format!("Invalid UTC ICS timestamp '{}': {}", raw, e))?;
        return Ok(ParsedTime::Timed(parsed.and_utc()));
    }

    let naive = NaiveDateTime::parse_from_str(raw, "%Y%m%dT%H%M%S")
        .map_err(|e| format!("Invalid ICS timestamp '{}': {}", raw, e))?;

    if let Some(tzid) = tzid.as_deref() {
        if let Some(timezone) = parse_timezone(tzid) {
            let localized = timezone
                .from_local_datetime(&naive)
                .single()
                .or_else(|| timezone.from_local_datetime(&naive).earliest())
                .ok_or_else(|| format!("Ambiguous ICS timezone '{}'", tzid))?;
            return Ok(ParsedTime::Timed(localized.with_timezone(&Utc)));
        }
        log::warn!(
            "Unsupported ICS TZID, defaulting to UTC: tzid={}",
            redact_tzid(tzid)
        );
    }

    Ok(ParsedTime::Timed(naive.and_utc()))
}

fn property_param(property: &ical::property::Property, key: &str) -> Option<String> {
    property.params.as_ref().and_then(|params| {
        params.iter().find_map(|(name, values)| {
            if name.eq_ignore_ascii_case(key) {
                values.first().cloned()
            } else {
                None
            }
        })
    })
}

fn parse_timezone(tzid: &str) -> Option<Tz> {
    Tz::from_str(tzid).ok().or_else(|| match tzid {
        "Central Standard Time" => Some(chrono_tz::America::Mexico_City),
        "Eastern Standard Time" => Some(chrono_tz::America::New_York),
        "Pacific Standard Time" => Some(chrono_tz::America::Los_Angeles),
        _ => None,
    })
}

fn redact_tzid(tzid: &str) -> String {
    tzid.chars().take(32).collect()
}

fn is_until_exceeded(current: DateTime<Utc>, until: Option<DateTime<Utc>>) -> bool {
    until.map(|until| current > until).unwrap_or(false)
}

#[derive(Debug, Clone)]
struct ParsedRRule {
    freq: String,
    interval: u32,
    count: Option<usize>,
    until: Option<DateTime<Utc>>,
    byday: Vec<Weekday>,
}

impl ParsedRRule {
    fn parse(raw: &str) -> Result<Self, String> {
        let mut fields = HashMap::new();
        for part in raw.split(';') {
            let mut pieces = part.splitn(2, '=');
            let key = pieces
                .next()
                .unwrap_or_default()
                .trim()
                .to_ascii_uppercase();
            let value = pieces.next().unwrap_or_default().trim().to_string();
            fields.insert(key, value);
        }

        let freq = fields
            .remove("FREQ")
            .filter(|value| !value.is_empty())
            .ok_or_else(|| "RRULE missing FREQ".to_string())?;
        let interval = fields
            .remove("INTERVAL")
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(1);
        let count = fields
            .remove("COUNT")
            .and_then(|value| value.parse::<usize>().ok());
        let until = fields
            .remove("UNTIL")
            .map(|value| parse_until(&value))
            .transpose()?;
        let byday = fields
            .remove("BYDAY")
            .map(|value| {
                value
                    .split(',')
                    .filter_map(parse_weekday)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        Ok(Self {
            freq,
            interval,
            count,
            until,
            byday,
        })
    }
}

fn parse_until(value: &str) -> Result<DateTime<Utc>, String> {
    if value.ends_with('Z') {
        DateTime::parse_from_str(value, "%Y%m%dT%H%M%SZ")
            .map(|value| value.with_timezone(&Utc))
            .map_err(|e| format!("Invalid RRULE UNTIL '{}': {}", value, e))
    } else if value.len() == 8 {
        let date = NaiveDate::parse_from_str(value, "%Y%m%d")
            .map_err(|e| format!("Invalid RRULE UNTIL date '{}': {}", value, e))?;
        Ok(date
            .and_hms_opt(23, 59, 59)
            .expect("valid all-day until")
            .and_utc())
    } else {
        let dt = NaiveDateTime::parse_from_str(value, "%Y%m%dT%H%M%S")
            .map_err(|e| format!("Invalid RRULE UNTIL timestamp '{}': {}", value, e))?;
        Ok(dt.and_utc())
    }
}

fn parse_weekday(value: &str) -> Option<Weekday> {
    match value {
        "MO" => Some(Weekday::Mon),
        "TU" => Some(Weekday::Tue),
        "WE" => Some(Weekday::Wed),
        "TH" => Some(Weekday::Thu),
        "FR" => Some(Weekday::Fri),
        "SA" => Some(Weekday::Sat),
        "SU" => Some(Weekday::Sun),
        _ => None,
    }
}

#[derive(Debug, Clone)]
enum ParsedTime {
    Timed(DateTime<Utc>),
    AllDay(NaiveDate),
}

#[cfg(test)]
mod tests {
    use super::{ParsedTime, parse_property_time_with_override};

    #[test]
    fn parses_utc_ics_timestamp_with_z_suffix() {
        let property = ical::property::Property {
            name: "DTSTART".to_string(),
            params: None,
            value: Some("20260204T190000Z".to_string()),
        };

        let parsed = parse_property_time_with_override(&property, "20260204T190000Z")
            .expect("valid utc timestamp");

        match parsed {
            ParsedTime::Timed(dt) => {
                assert_eq!(dt.to_rfc3339(), "2026-02-04T19:00:00+00:00");
            }
            ParsedTime::AllDay(_) => panic!("expected timed value"),
        }
    }
}
