use crate::tools::gemini::run_workspace_command;

pub async fn fetch_calendar_summary_raw() -> Result<Option<String>, String> {
    let prompt = "/calendar:get-schedule today";

    run_workspace_command(prompt)
}
