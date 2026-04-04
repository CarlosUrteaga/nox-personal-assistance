# nox-personal-assistance

Telegram-based personal assistant backed by Ollama, with a local web console, guided setup, a privacy-first calendar heartbeat, and a scheduled AI agents / LLMOps / RAG news brief.

## What it does

- Receives plain text messages in Telegram
- Sends them to an Ollama chat model
- Keeps a short in-memory conversation history per chat
- Stores internal todos locally in JSON
- Exposes a local web interface with registration, login, guided setup, a run console, and `.env` editing
- Supports `/start`, `/help`, `/reset`, `/todo <task>`, `/todos`, and `/done <id>`
- Also handles common natural todo phrases before falling back to chat
- Optionally polls multiple ICS/iCal feeds and writes only generic busy blockers into a destination Google Calendar
- Optionally sends a scheduled AI agents / LLMOps / RAG RSS brief to Telegram at configured local-time windows

## Requirements

- Rust toolchain
- Ollama running locally or remotely
- A Telegram bot token
- The numeric Telegram chat ID that should be allowed to talk to the bot
- For calendar sync: one destination Google Calendar and either Google OAuth `credentials.json` plus a generated `token.json`, or a valid bearer token with event read/write scope

## Configuration

Copy `.env.example` to `.env` and set:

- `TELOXIDE_TOKEN`
- `CHAT_ID`
- `OLLAMA_BASE_URL`
- `OLLAMA_MODEL`
- `OLLAMA_TIMEOUT_SECS`
- `OLLAMA_NUM_PREDICT`
- `ASSISTANT_NAME`
- `SYSTEM_PROMPT`
- `MAX_HISTORY_MESSAGES`
- `TODO_STORE_PATH`
- `NEWS_BRIEF_ENABLED`
- `NEWS_BRIEF_TIMEZONE`
- `NEWS_BRIEF_SCHEDULE_JSON`
- `NEWS_BRIEF_MAX_ITEMS`
- `NEWS_BRIEF_MIN_ITEMS`
- `NEWS_BRIEF_MIN_AVG_SCORE`
- `NEWS_BRIEF_LOOKBACK_HOURS`
- `NEWS_BRIEF_FETCH_COOLDOWN_MINUTES`
- `NEWS_BRIEF_MAX_SUMMARY_CHARS`
- `NEWS_BRIEF_STORE_PATH`
- `NEWS_BRIEF_SOURCES_JSON`
- `NEWS_BRIEF_ENABLED_TOPICS_JSON`
- `NEWS_BRIEF_NEGATIVE_KEYWORDS_JSON`
- `WEB_ENABLED`
- `WEB_BIND_ADDRESS`
- `USER_STORE_PATH`
- `DESTINATION_CALENDAR_ID`
- `CALENDAR_DESTINATION_PROVIDER`
- `GOOGLE_OAUTH_CREDENTIALS_PATH`
- `GOOGLE_OAUTH_TOKEN_PATH`
- `GOOGLE_CALENDAR_ACCESS_TOKEN`
- `HEARTBEAT_INTERVAL_SECS`
- `HEARTBEAT_SYNC_WINDOW_DAYS`
- `CALENDAR_TARGET_EMAILS`
- `CALENDAR_SOURCES_JSON`

Calendar env notes:

- `CALENDAR_TARGET_EMAILS` must be a JSON array in a single line, wrapped in quotes in `.env`.
- `CALENDAR_SOURCES_JSON` must be a single-line JSON array in `.env`.
- Each source should define `owner_email` so invitation routing can exclude the source owner automatically.
- `GOOGLE_OAUTH_TOKEN_PATH` is optional. If omitted, NOX stores `token.json` next to `credentials.json`.

News brief env notes:

- `NEWS_BRIEF_SOURCES_JSON` must be a single-line JSON array in `.env`.
- `NEWS_BRIEF_SCHEDULE_JSON` must be a JSON array of `HH:MM` local-time slots.
- `NEWS_BRIEF_ENABLED_TOPICS_JSON` defaults to `agents`, `llmops`, and `rag`.
- The scheduler sends at most one brief per configured window and stores state in `NEWS_BRIEF_STORE_PATH`.

## Run

```bash
cargo run
```

With the defaults above, the web UI listens on `http://127.0.0.1:3000`.
If the Telegram configuration is incomplete, NOX still starts the web UI so you can finish setup there.

Web auth notes:

- The first user can register locally through the site.
- Passwords are stored in `USER_STORE_PATH` as Argon2 hashes, never in `.env`.
- Sessions use random server-side tokens in `HttpOnly` cookies.
- The site edits the shared `.env`, so all registered users currently manage the same global configuration.
- Changes saved in the UI are written to `.env`; restart the process to apply values already loaded in memory.

Setup notes:

- After registration, the user is redirected to a guided `/setup` flow.
- The initial setup asks only for the minimum required values:
  - `TELOXIDE_TOKEN`
  - `CHAT_ID`
  - `OLLAMA_BASE_URL`
  - `OLLAMA_MODEL`
- The setup page includes short instructions for how to obtain each required value.
- The setup can be skipped, but the console will keep showing a configuration warning banner until the required values exist.
- Advanced or optional configuration remains available in `/settings`.

Console notes:

- The authenticated home route is now a run-oriented console instead of a plain settings page.
- Real Telegram requests are rendered as run cards with status, steps, metadata, final result, and optional error context.
- The console currently tracks Telegram-originated runtime activity.
- If there are no real runs yet, the console shows a single clearly marked demo run as a fallback example.
- The UI supports `simple` and `detailed` viewing modes and the data model is ready for future streaming and tool traces.

If `CALENDAR_SOURCES_JSON` is configured with at least one enabled source, the process also starts a periodic heartbeat that:

- Fetches each enabled ICS source
- Normalizes events into privacy-safe internal timings
- Resolves overlaps by configured priority
- Creates, updates, or deletes only generic blockers in `DESTINATION_CALENDAR_ID`
- Sends invite updates to configured `CALENDAR_TARGET_EMAILS`, excluding the winning source `owner_email`
- Sends Telegram messages only when the heartbeat fails or when blockers changed

Manual CLI commands:

```bash
cargo run -- calendar-sync
cargo run -- calendar-sync --dry-run
cargo run -- google-auth
```

- `calendar-sync` runs a one-off reconcile without starting Telegram.
- `calendar-sync --dry-run` fetches sources and resolves blockers without writing to the destination calendar.
- `google-auth` bootstraps `token.json` from `credentials.json` using a one-time browser consent flow.

Example calendar config for `.env`:

```env
CALENDAR_DESTINATION_PROVIDER=google
WEB_ENABLED=true
WEB_BIND_ADDRESS=127.0.0.1:3000
USER_STORE_PATH=data/users.json
DESTINATION_CALENDAR_ID=primary
GOOGLE_OAUTH_CREDENTIALS_PATH=/absolute/path/to/credentials.json
HEARTBEAT_INTERVAL_SECS=1800
HEARTBEAT_SYNC_WINDOW_DAYS=14
CALENDAR_TARGET_EMAILS='["personal@example.com","work@example.com","client@example.com"]'
CALENDAR_SOURCES_JSON='[{"id":"work","type":"ics","url":"https://example.test/work.ics","label":"Busy - Work","priority":80,"category":"business","enabled":true,"owner_email":"work@example.com"},{"id":"client","type":"ics","url":"https://example.test/client.ics","label":"Busy - Client","priority":100,"category":"business","enabled":true,"owner_email":"client@example.com"},{"id":"personal","type":"ics","url":"https://example.test/personal.ics","label":"Busy - Personal","priority":60,"category":"personal","enabled":true,"owner_email":"personal@example.com"}]'
```

Recommended Google auth setup from `credentials.json`:

1. In Google Cloud, enable `Google Calendar API`.
2. Create an OAuth client for a desktop app.
3. Download the OAuth client file as `credentials.json`.
4. Add the Google account you will use as a test user if the consent screen is still in testing mode.
5. Point NOX to `credentials.json`. Optionally override the token location:

```env
GOOGLE_OAUTH_CREDENTIALS_PATH=/absolute/path/to/credentials.json
GOOGLE_OAUTH_TOKEN_PATH=/absolute/path/to/token.json
```

If `GOOGLE_OAUTH_TOKEN_PATH` is omitted, NOX uses `/absolute/path/to/token.json` next to `credentials.json`.

6. Run the bootstrap command:

```bash
cargo run -- google-auth
```

7. Open the printed Google consent URL in your browser.
8. After Google redirects back to the local callback, NOX writes `token.json`.
9. Run NOX normally. From then on, NOX uses `token.json`, refreshes access tokens automatically, and can recreate `token.json` if it is missing, invalid, or no longer refreshable.

If you want to validate the current token before running the full app:

```bash
ACCESS_TOKEN=$(python -c 'from google.oauth2.credentials import Credentials; print(Credentials.from_authorized_user_file("token.json", ["https://www.googleapis.com/auth/calendar"]).token)')
curl https://www.googleapis.com/calendar/v3/users/me/calendarList \
  -H "Authorization: Bearer ${ACCESS_TOKEN}"
```

Legacy manual bearer-token setup:

1. Open this authorization URL in a browser, replacing `YOUR_CLIENT_ID`:

```text
https://accounts.google.com/o/oauth2/v2/auth?client_id=YOUR_CLIENT_ID&redirect_uri=http://127.0.0.1:8080&response_type=code&scope=https://www.googleapis.com/auth/calendar&access_type=offline&prompt=consent
```

2. After approving access, copy the `code` query parameter from the redirect URL.
3. Exchange that code for tokens:

```bash
curl -X POST https://oauth2.googleapis.com/token \
  -H "Content-Type: application/x-www-form-urlencoded" \
  -d "client_id=YOUR_CLIENT_ID" \
  -d "client_secret=YOUR_CLIENT_SECRET" \
  -d "code=AUTHORIZATION_CODE" \
  -d "grant_type=authorization_code" \
  -d "redirect_uri=http://127.0.0.1:8080"
```

4. Copy `access_token` from the JSON response into `GOOGLE_CALENDAR_ACCESS_TOKEN`.
5. Validate the token before running NOX:

```bash
curl https://www.googleapis.com/calendar/v3/users/me/calendarList \
  -H "Authorization: Bearer YOUR_ACCESS_TOKEN"
```

Notes:

- `CALENDAR_DESTINATION_PROVIDER` currently supports `google`.
- `GOOGLE_OAUTH_CREDENTIALS_PATH` is used for the first-time `cargo run -- google-auth` bootstrap flow and for automatic runtime recovery when a token file is missing or unusable.
- `GOOGLE_OAUTH_TOKEN_PATH` is an optional override for the runtime credential file location. If omitted, NOX uses `token.json` beside `credentials.json`.
- NOX reads the runtime token file, reuses the stored `refresh_token`, and refreshes the access token automatically when needed.
- `GOOGLE_CALENDAR_ACCESS_TOKEN` remains supported as a legacy fallback.
- `credentials.json` alone is not a runtime credential, but it is enough for NOX to generate or recover `token.json`.
- If `token.json` is missing, malformed, or revoked later, NOX attempts the same browser-based recovery flow automatically. `cargo run -- google-auth` remains available as a manual recovery command.
- Treat the access token, refresh token, client secret, and private ICS URLs as secrets.

## Notes

- Conversation memory is process-local and resets when the app restarts.
- Todos persist locally in the path configured by `TODO_STORE_PATH`.
- Source calendar details are never copied into the destination blocker events. Only generic configured labels are written.
- Invitation routing uses the configured generic blocker plus target emails only; source meeting metadata is never copied.
- The Google Calendar sync path prefers token-file OAuth auth and falls back to `GOOGLE_CALENDAR_ACCESS_TOKEN`.
- Legacy Google Workspace and `gemini` CLI integrations remain outside the active runtime flow.
- Natural todo examples: `add todo buy milk`, `show my todos`, `complete 2`, `remember to pay rent`.
