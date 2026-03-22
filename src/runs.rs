use chrono::Utc;
use std::sync::{
    Arc, RwLock,
    atomic::{AtomicU64, Ordering},
};

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
}

impl RunStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Queued => "Queued",
            Self::Running => "Running",
            Self::Succeeded => "Succeeded",
            Self::Failed => "Failed",
        }
    }

    pub fn tone_class(self) -> &'static str {
        match self {
            Self::Queued => "neutral",
            Self::Running => "info",
            Self::Succeeded => "success",
            Self::Failed => "danger",
        }
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Skipped,
}

impl StepStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Pending => "Pending",
            Self::Running => "Running",
            Self::Completed => "Completed",
            Self::Failed => "Failed",
            Self::Skipped => "Skipped",
        }
    }

    pub fn tone_class(self) -> &'static str {
        match self {
            Self::Pending => "neutral",
            Self::Running => "info",
            Self::Completed => "success",
            Self::Failed => "danger",
            Self::Skipped => "muted",
        }
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepKind {
    Input,
    Planning,
    Tool,
    Retrieval,
    Reasoning,
    Output,
    Validation,
}

impl StepKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Input => "Input",
            Self::Planning => "Plan",
            Self::Tool => "Tool",
            Self::Retrieval => "Retrieve",
            Self::Reasoning => "Reason",
            Self::Output => "Output",
            Self::Validation => "Validate",
        }
    }
}

#[derive(Debug, Clone)]
pub struct MetadataItem {
    pub label: String,
    pub value: String,
}

impl MetadataItem {
    pub fn new(label: &str, value: &str) -> Self {
        Self {
            label: label.to_string(),
            value: value.to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ToolTrace {
    pub label: String,
    pub payload: String,
}

#[derive(Debug, Clone)]
pub struct RunError {
    pub title: String,
    pub message: String,
    pub suggestion: String,
}

#[derive(Debug, Clone)]
pub struct RunResult {
    pub title: String,
    pub summary: String,
    pub highlights: Vec<String>,
    pub body: Vec<String>,
    pub artifacts: Vec<MetadataItem>,
}

impl RunResult {
    pub fn text(title: &str, summary: &str, content: &str) -> Self {
        Self {
            title: title.to_string(),
            summary: summary.to_string(),
            highlights: Vec::new(),
            body: vec![content.to_string()],
            artifacts: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Step {
    pub id: String,
    pub title: String,
    pub status: StepStatus,
    pub kind: StepKind,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub summary: String,
    pub detail: String,
    pub trace: Option<ToolTrace>,
    pub metrics: Vec<MetadataItem>,
}

#[derive(Debug, Clone)]
pub struct Run {
    pub id: String,
    pub request_title: String,
    pub request_text: String,
    pub summary: String,
    pub status: RunStatus,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub latency_ms: u64,
    pub step_count: usize,
    pub active_step_label: Option<String>,
    pub conversation_mode: String,
    pub model: String,
    pub channel: String,
    pub final_result: RunResult,
    pub error: Option<RunError>,
    pub metadata: Vec<MetadataItem>,
    pub steps: Vec<Step>,
    pub is_demo: bool,
    started_at_epoch_ms: u64,
}

#[derive(Debug, Clone)]
pub struct RunDraft {
    pub request_title: String,
    pub request_text: String,
    pub summary: String,
    pub conversation_mode: String,
    pub model: String,
    pub channel: String,
    pub metadata: Vec<MetadataItem>,
}

#[derive(Debug, Clone)]
pub struct StepDraft {
    pub kind: StepKind,
    pub title: String,
    pub summary: String,
}

#[derive(Debug, Clone, Default)]
struct RunTrackerState {
    runs: Vec<Run>,
}

#[derive(Clone, Default)]
pub struct RunTracker {
    inner: Arc<RwLock<RunTrackerState>>,
    next_run_id: Arc<AtomicU64>,
    next_step_id: Arc<AtomicU64>,
}

impl RunTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn start_run(&self, draft: RunDraft) -> String {
        let now = now_epoch_ms();
        let run_id = format!(
            "run_{:020}",
            self.next_run_id.fetch_add(1, Ordering::Relaxed)
        );
        let run = Run {
            id: run_id.clone(),
            request_title: draft.request_title,
            request_text: draft.request_text,
            summary: draft.summary,
            status: RunStatus::Running,
            started_at: format_timestamp(now),
            finished_at: None,
            latency_ms: 0,
            step_count: 0,
            active_step_label: None,
            conversation_mode: draft.conversation_mode,
            model: draft.model,
            channel: draft.channel,
            final_result: RunResult::text(
                "In progress",
                "Run started",
                "The request is executing.",
            ),
            error: None,
            metadata: draft.metadata,
            steps: Vec::new(),
            is_demo: false,
            started_at_epoch_ms: now,
        };

        if let Ok(mut inner) = self.inner.write() {
            inner.runs.insert(0, run);
        }

        run_id
    }

    pub fn start_step(&self, run_id: &str, draft: StepDraft) -> String {
        let step_id = format!(
            "step_{:020}",
            self.next_step_id.fetch_add(1, Ordering::Relaxed)
        );
        let now = format_timestamp(now_epoch_ms());
        self.mutate_run(run_id, |run| {
            run.active_step_label = Some(draft.title.clone());
            run.steps.push(Step {
                id: step_id.clone(),
                title: draft.title,
                status: StepStatus::Running,
                kind: draft.kind,
                started_at: now,
                finished_at: None,
                summary: draft.summary,
                detail: String::new(),
                trace: None,
                metrics: Vec::new(),
            });
            run.step_count = run.steps.len();
        });
        step_id
    }

    pub fn finish_step(
        &self,
        run_id: &str,
        step_id: &str,
        summary: &str,
        detail: &str,
        trace: Option<ToolTrace>,
        metrics: Vec<MetadataItem>,
    ) {
        let finished_at = format_timestamp(now_epoch_ms());
        self.mutate_run(run_id, |run| {
            if let Some(step) = run.steps.iter_mut().find(|step| step.id == step_id) {
                step.status = StepStatus::Completed;
                step.summary = summary.to_string();
                step.detail = detail.to_string();
                step.trace = trace;
                step.metrics = metrics;
                step.finished_at = Some(finished_at.clone());
            }
        });
    }

    pub fn fail_step(
        &self,
        run_id: &str,
        step_id: &str,
        summary: &str,
        detail: &str,
        trace: Option<ToolTrace>,
        metrics: Vec<MetadataItem>,
    ) {
        let finished_at = format_timestamp(now_epoch_ms());
        self.mutate_run(run_id, |run| {
            if let Some(step) = run.steps.iter_mut().find(|step| step.id == step_id) {
                step.status = StepStatus::Failed;
                step.summary = summary.to_string();
                step.detail = detail.to_string();
                step.trace = trace;
                step.metrics = metrics;
                step.finished_at = Some(finished_at.clone());
            }
        });
    }

    pub fn complete_run(
        &self,
        run_id: &str,
        summary: &str,
        final_result: RunResult,
        metadata: Vec<MetadataItem>,
    ) {
        let now = now_epoch_ms();
        self.mutate_run(run_id, |run| {
            run.status = RunStatus::Succeeded;
            run.summary = summary.to_string();
            run.final_result = final_result;
            run.metadata.extend(metadata);
            run.finished_at = Some(format_timestamp(now));
            run.latency_ms = now.saturating_sub(run.started_at_epoch_ms);
            run.active_step_label = None;
            run.step_count = run.steps.len();
            run.error = None;
        });
    }

    pub fn fail_run(
        &self,
        run_id: &str,
        summary: &str,
        error: RunError,
        final_result: RunResult,
        metadata: Vec<MetadataItem>,
    ) {
        let now = now_epoch_ms();
        self.mutate_run(run_id, |run| {
            run.status = RunStatus::Failed;
            run.summary = summary.to_string();
            run.error = Some(error);
            run.final_result = final_result;
            run.metadata.extend(metadata);
            run.finished_at = Some(format_timestamp(now));
            run.latency_ms = now.saturating_sub(run.started_at_epoch_ms);
            run.active_step_label = None;
            run.step_count = run.steps.len();
        });
    }

    pub fn list_runs_with_fallback(&self) -> Vec<Run> {
        let runs = self
            .inner
            .read()
            .map(|inner| inner.runs.clone())
            .unwrap_or_default();
        if runs.is_empty() {
            vec![fallback_demo_run()]
        } else {
            runs
        }
    }

    fn mutate_run(&self, run_id: &str, mutator: impl FnOnce(&mut Run)) {
        if let Ok(mut inner) = self.inner.write() {
            if let Some(run) = inner.runs.iter_mut().find(|run| run.id == run_id) {
                mutator(run);
            }
        }
    }
}

fn now_epoch_ms() -> u64 {
    Utc::now().timestamp_millis().max(0) as u64
}

fn format_timestamp(epoch_ms: u64) -> String {
    let seconds = (epoch_ms / 1000) as i64;
    let nanos = ((epoch_ms % 1000) * 1_000_000) as u32;
    chrono::DateTime::<Utc>::from_timestamp(seconds, nanos)
        .map(|dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

fn fallback_demo_run() -> Run {
    let now = now_epoch_ms();
    Run {
        id: "demo_run_console".to_string(),
        request_title: "Welcome to the NOX console".to_string(),
        request_text: "This is a demo run shown only when the runtime has not produced any real executions yet.".to_string(),
        summary: "Real Telegram requests will replace this placeholder automatically as soon as the first run is created.".to_string(),
        status: RunStatus::Succeeded,
        started_at: format_timestamp(now),
        finished_at: Some(format_timestamp(now)),
        latency_ms: 640,
        step_count: 3,
        active_step_label: None,
        conversation_mode: "Demo".to_string(),
        model: "n/a".to_string(),
        channel: "Console".to_string(),
        final_result: RunResult {
            title: "Console ready".to_string(),
            summary: "The runtime-backed viewer is active. This demo exists only to explain the layout before the first real request arrives.".to_string(),
            highlights: vec![
                "Each Telegram request will appear here as a run card.".to_string(),
                "Runs expose steps, metadata, final result and failure context.".to_string(),
                "This demo disappears automatically after the first real run.".to_string(),
            ],
            body: vec![
                "Use the setup flow to finish configuration, then send a Telegram message to see a real run enter the console.".to_string(),
            ],
            artifacts: vec![MetadataItem::new("Mode", "Demo fallback only")],
        },
        error: None,
        metadata: vec![
            MetadataItem::new("Run ID", "demo_run_console"),
            MetadataItem::new("Source", "Fallback demo"),
        ],
        steps: vec![
            Step {
                id: "demo_step_1".to_string(),
                title: "Render console".to_string(),
                status: StepStatus::Completed,
                kind: StepKind::Input,
                started_at: format_timestamp(now),
                finished_at: Some(format_timestamp(now)),
                summary: "Loaded the console shell.".to_string(),
                detail: "The web UI can render without any runtime activity.".to_string(),
                trace: None,
                metrics: Vec::new(),
            },
            Step {
                id: "demo_step_2".to_string(),
                title: "Explain observable runs".to_string(),
                status: StepStatus::Completed,
                kind: StepKind::Planning,
                started_at: format_timestamp(now),
                finished_at: Some(format_timestamp(now)),
                summary: "Outlined what a real run will look like.".to_string(),
                detail: "Future runtime executions will replace this card in the primary history list.".to_string(),
                trace: None,
                metrics: Vec::new(),
            },
            Step {
                id: "demo_step_3".to_string(),
                title: "Show final result".to_string(),
                status: StepStatus::Completed,
                kind: StepKind::Output,
                started_at: format_timestamp(now),
                finished_at: Some(format_timestamp(now)),
                summary: "Presented the final result panel.".to_string(),
                detail: "This mirrors the structure used for real runs.".to_string(),
                trace: None,
                metrics: Vec::new(),
            },
        ],
        is_demo: true,
        started_at_epoch_ms: now,
    }
}

#[cfg(test)]
mod tests {
    use super::{MetadataItem, RunDraft, RunResult, RunStatus, RunTracker, StepDraft, StepKind};

    #[test]
    fn returns_demo_when_empty() {
        let tracker = RunTracker::new();
        let runs = tracker.list_runs_with_fallback();
        assert_eq!(runs.len(), 1);
        assert!(runs[0].is_demo);
    }

    #[test]
    fn real_runs_replace_demo() {
        let tracker = RunTracker::new();
        let run_id = tracker.start_run(RunDraft {
            request_title: "Hello".to_string(),
            request_text: "hello".to_string(),
            summary: "running".to_string(),
            conversation_mode: "Chat".to_string(),
            model: "test".to_string(),
            channel: "Telegram".to_string(),
            metadata: vec![MetadataItem::new("chat_id", "1")],
        });
        let step_id = tracker.start_step(
            &run_id,
            StepDraft {
                kind: StepKind::Input,
                title: "Input".to_string(),
                summary: "received".to_string(),
            },
        );
        tracker.finish_step(&run_id, &step_id, "received", "received", None, Vec::new());
        tracker.complete_run(
            &run_id,
            "done",
            RunResult::text("Done", "done", "done"),
            Vec::new(),
        );

        let runs = tracker.list_runs_with_fallback();
        assert_eq!(runs.len(), 1);
        assert!(!runs[0].is_demo);
        assert_eq!(runs[0].status, RunStatus::Succeeded);
    }
}
