use crate::calendar::service::CalendarSyncOutcome;

pub fn format_calendar_sync_summary(outcome: &CalendarSyncOutcome) -> String {
    format!(
        "Calendar sync finished.\nSource events: {}\nResolved blockers: {}\nCreated: {}\nUpdated: {}\nCancelled: {}",
        outcome.source_events,
        outcome.blockers,
        outcome.stats.created,
        outcome.stats.updated,
        outcome.stats.deleted
    )
}

pub fn format_calendar_sync_cli(outcome: &CalendarSyncOutcome) -> String {
    format!(
        "calendar-sync finished\nsource_events={}\nresolved_blockers={}\ncreated={}\nupdated={}\ndeleted={}",
        outcome.source_events,
        outcome.blockers,
        outcome.stats.created,
        outcome.stats.updated,
        outcome.stats.deleted
    )
}

pub fn format_calendar_sync_dry_run_cli(outcome: &CalendarSyncOutcome) -> String {
    format!(
        "calendar-sync dry-run\nsource_events={}\nresolved_blockers={}\ncreated=0\nupdated=0\ndeleted=0",
        outcome.source_events,
        outcome.blockers,
    )
}

pub fn format_calendar_heartbeat_success(outcome: &CalendarSyncOutcome) -> String {
    format!(
        "NOX heartbeat updated blockers.\nSource events: {}\nResolved blockers: {}\nCreated: {}\nUpdated: {}\nCancelled: {}",
        outcome.source_events,
        outcome.blockers,
        outcome.stats.created,
        outcome.stats.updated,
        outcome.stats.deleted
    )
}

pub fn format_calendar_heartbeat_error(error: &str) -> String {
    format!("NOX heartbeat failed.\n{}", sanitize_error(error))
}

fn sanitize_error(error: &str) -> String {
    error.chars().take(280).collect()
}
