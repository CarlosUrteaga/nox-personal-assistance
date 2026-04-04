use crate::channels::telegram::TelegramChannel;
use crate::config::AppConfig;
use crate::runs::{MetadataItem, RunResult, StepKind, ToolTrace};
use crate::tools::news::{GenerateOutcome, NewsBriefService, NewsRunRecorder};
use chrono::{Datelike, TimeZone, Timelike, Utc};

pub struct NewsScheduler {
    service: NewsBriefService,
    telegram: TelegramChannel,
    recorder: NewsRunRecorder,
}

impl NewsScheduler {
    pub fn new(
        config: AppConfig,
        telegram: TelegramChannel,
        run_tracker: crate::runs::RunTracker,
    ) -> Result<Self, String> {
        let service = NewsBriefService::new(config)?;
        Ok(Self {
            service,
            telegram,
            recorder: NewsRunRecorder::new(run_tracker),
        })
    }

    pub async fn run(self) {
        let timezone = match self.service.timezone() {
            Ok(timezone) => timezone,
            Err(err) => {
                log::error!("News scheduler disabled: {}", err);
                return;
            }
        };
        let schedule_times = match self.service.schedule_times() {
            Ok(times) => times,
            Err(err) => {
                log::error!("News scheduler disabled: {}", err);
                return;
            }
        };

        let mut interval = tokio::time::interval(std::time::Duration::from_secs(20));
        loop {
            interval.tick().await;

            let now_local = Utc::now().with_timezone(&timezone);
            let Some(scheduled_time) = schedule_times
                .iter()
                .find(|time| time.hour() == now_local.hour() && time.minute() == now_local.minute())
                .copied()
            else {
                continue;
            };

            let Some(scheduled_at) = timezone
                .with_ymd_and_hms(
                    now_local.year(),
                    now_local.month(),
                    now_local.day(),
                    scheduled_time.hour(),
                    scheduled_time.minute(),
                    0,
                )
                .single()
            else {
                continue;
            };

            let window_key = scheduled_at.format("%Y-%m-%d %H:%M").to_string();
            if matches!(self.service.window_status(&window_key).await, Ok(Some(_))) {
                continue;
            }

            self.execute_window(scheduled_at).await;
        }
    }

    async fn execute_window(&self, scheduled_at: chrono::DateTime<chrono_tz::Tz>) {
        let window_key = scheduled_at.format("%Y-%m-%d %H:%M").to_string();
        let run_id = self.recorder.start(&window_key);

        let generate_step = self.recorder.start_step(
            &run_id,
            StepKind::Planning,
            "Generate news artifact",
            "Fetching sources and building the scheduled news artifact.",
        );

        let generated = match self.service.generate_news_brief(scheduled_at).await {
            Ok(outcome) => outcome,
            Err(err) => {
                self.recorder.fail_step(
                    &run_id,
                    &generate_step,
                    "News brief generation failed.",
                    "The generator failed before a prepared artifact could be produced.",
                );
                self.recorder.fail(
                    &run_id,
                    "News brief generation failed.",
                    "News brief failed",
                    &err,
                    "Check feed availability, source configuration, and network connectivity.",
                );
                return;
            }
        };

        match generated {
            GenerateOutcome::Skipped { reason, metrics } => {
                self.recorder.finish_step(
                    &run_id,
                    &generate_step,
                    "News brief skipped.",
                    "Generation completed, but the delivery eligibility gate rejected the artifact.",
                    None,
                    vec![
                        MetadataItem::new("selected_count", &metrics.selected_count.to_string()),
                        MetadataItem::new("relevant_count", &metrics.relevant_count.to_string()),
                    ],
                );
                self.recorder.complete(
                    &run_id,
                    "News brief skipped by delivery eligibility gate.",
                    RunResult::text("News brief skipped", "No send was performed for this window.", &reason),
                    vec![
                        MetadataItem::new("status", "skipped"),
                        MetadataItem::new("window_key", &window_key),
                    ],
                );
            }
            GenerateOutcome::Prepared(brief) => {
                self.recorder.finish_step(
                    &run_id,
                    &generate_step,
                    "News brief artifact prepared.",
                    "Fetched sources, scored candidates, and persisted the prepared artifact.",
                    Some(ToolTrace {
                        label: "news.prepared_artifact_hash".to_string(),
                        payload: brief.prepared_artifact_hash.clone(),
                    }),
                    vec![
                        MetadataItem::new("selected_count", &brief.metrics.selected_count.to_string()),
                        MetadataItem::new("relevant_count", &brief.metrics.relevant_count.to_string()),
                        MetadataItem::new("prepared_artifact_hash", &brief.prepared_artifact_hash),
                    ],
                );

                let send_step = self.recorder.start_step(
                    &run_id,
                    StepKind::Output,
                    "Send Telegram brief",
                    "Sending the prepared artifact to Telegram.",
                );
                match self.service.send_news_brief(&self.telegram, &brief).await {
                    Ok(()) => {
                        self.recorder.finish_step(
                            &run_id,
                            &send_step,
                            "Telegram brief sent.",
                            "The prepared artifact was delivered successfully through the Telegram channel.",
                            None,
                            vec![MetadataItem::new("sent_count", "1")],
                        );
                        self.recorder.complete(
                            &run_id,
                            "News brief prepared and sent successfully.",
                            RunResult::text(
                                "News brief sent",
                                "The scheduled Telegram brief was delivered.",
                                &format!(
                                    "Window: {}\nTop titles: {}",
                                    window_key,
                                    brief
                                        .items
                                        .iter()
                                        .map(|item| item.title.as_str())
                                        .collect::<Vec<_>>()
                                        .join(" | ")
                                ),
                            ),
                            vec![
                                MetadataItem::new("status", "sent"),
                                MetadataItem::new("window_key", &window_key),
                                MetadataItem::new("selected_count", &brief.metrics.selected_count.to_string()),
                            ],
                        );
                    }
                    Err(err) => {
                        self.recorder.fail_step(
                            &run_id,
                            &send_step,
                            "Telegram delivery failed.",
                            "The prepared artifact was preserved, but Telegram delivery failed.",
                        );
                        self.recorder.fail(
                            &run_id,
                            "News brief delivery failed.",
                            "News brief delivery failed",
                            &err,
                            "Inspect the Telegram bot token, target chat, and network connectivity.",
                        );
                    }
                }
            }
        }
    }
}
