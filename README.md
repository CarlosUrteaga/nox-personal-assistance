# nox-personal-assistance

Telegram-based personal assistant backed by Ollama, with an optional privacy-first calendar heartbeat.

## What it does

- Receives plain text messages in Telegram
- Sends them to an Ollama chat model
- Keeps a short in-memory conversation history per chat
- Stores internal todos locally in JSON
- Supports `/start`, `/help`, `/reset`, `/todo <task>`, `/todos`, and `/done <id>`
- Also handles common natural todo phrases before falling back to chat
- Optionally polls multiple ICS/iCal feeds and writes only generic busy blockers into a destination Google Calendar

## Requirements

- Rust toolchain
- Ollama running locally or remotely
- A Telegram bot token
- The numeric Telegram chat ID that should be allowed to talk to the bot
- For calendar sync: one destination Google Calendar and a valid bearer token with event read/write scope

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
- `DESTINATION_CALENDAR_ID`
- `GOOGLE_CALENDAR_ACCESS_TOKEN`
- `HEARTBEAT_INTERVAL_SECS`
- `HEARTBEAT_SYNC_WINDOW_DAYS`
- `CALENDAR_TARGET_EMAILS`
- `CALENDAR_SOURCES_JSON`

Calendar env notes:

- `CALENDAR_TARGET_EMAILS` must be a JSON array in a single line, wrapped in quotes in `.env`.
- `CALENDAR_SOURCES_JSON` must be a single-line JSON array in `.env`.
- Each source should define `owner_email` so invitation routing can exclude the source owner automatically.

## Run

```bash
cargo run
```

If `CALENDAR_SOURCES_JSON` is configured with at least one enabled source, the process also starts a periodic heartbeat that:

- Fetches each enabled ICS source
- Normalizes events into privacy-safe internal timings
- Resolves overlaps by configured priority
- Creates, updates, or deletes only generic blockers in `DESTINATION_CALENDAR_ID`
- Sends invite updates to configured `CALENDAR_TARGET_EMAILS`, excluding the winning source `owner_email`
- Sends Telegram messages only when the heartbeat fails or when blockers changed

Example calendar config for `.env`:

```env
DESTINATION_CALENDAR_ID=primary
GOOGLE_CALENDAR_ACCESS_TOKEN=ya29...
HEARTBEAT_INTERVAL_SECS=1800
HEARTBEAT_SYNC_WINDOW_DAYS=14
CALENDAR_TARGET_EMAILS='["personal@example.com","work@example.com","client@example.com"]'
CALENDAR_SOURCES_JSON='[{"id":"work","type":"ics","url":"https://example.test/work.ics","label":"Busy - Work","priority":80,"category":"business","enabled":true,"owner_email":"work@example.com"},{"id":"client","type":"ics","url":"https://example.test/client.ics","label":"Busy - Client","priority":100,"category":"business","enabled":true,"owner_email":"client@example.com"},{"id":"personal","type":"ics","url":"https://example.test/personal.ics","label":"Busy - Personal","priority":60,"category":"personal","enabled":true,"owner_email":"personal@example.com"}]'
```

Getting a Google bearer token:

1. In Google Cloud, enable `Google Calendar API`.
2. Create an OAuth client for a desktop app.
3. Add the Google account you will use as a test user if the consent screen is still in testing mode.
4. Open this authorization URL in a browser, replacing `YOUR_CLIENT_ID`:

```text
https://accounts.google.com/o/oauth2/v2/auth?client_id=YOUR_CLIENT_ID&redirect_uri=http://127.0.0.1:8080&response_type=code&scope=https://www.googleapis.com/auth/calendar&access_type=offline&prompt=consent
```

5. After approving access, copy the `code` query parameter from the redirect URL.
6. Exchange that code for tokens:

```bash
curl -X POST https://oauth2.googleapis.com/token \
  -H "Content-Type: application/x-www-form-urlencoded" \
  -d "client_id=YOUR_CLIENT_ID" \
  -d "client_secret=YOUR_CLIENT_SECRET" \
  -d "code=AUTHORIZATION_CODE" \
  -d "grant_type=authorization_code" \
  -d "redirect_uri=http://127.0.0.1:8080"
```

7. Copy `access_token` from the JSON response into `GOOGLE_CALENDAR_ACCESS_TOKEN`.
8. Validate the token before running NOX:

```bash
curl https://www.googleapis.com/calendar/v3/users/me/calendarList \
  -H "Authorization: Bearer YOUR_ACCESS_TOKEN"
```

Notes:

- `GOOGLE_CALENDAR_ACCESS_TOKEN` expects the `access_token`, not the OAuth client ID or client secret.
- Access tokens expire. This project currently expects you to refresh or replace the token manually.
- Treat the access token, refresh token, client secret, and private ICS URLs as secrets.

## Notes

- Conversation memory is process-local and resets when the app restarts.
- Todos persist locally in the path configured by `TODO_STORE_PATH`.
- Source calendar details are never copied into the destination blocker events. Only generic configured labels are written.
- Invitation routing uses the configured generic blocker plus target emails only; source meeting metadata is never copied.
- The Google Calendar sync path currently expects a ready-to-use bearer token in `GOOGLE_CALENDAR_ACCESS_TOKEN`.
- Legacy Google Workspace and `gemini` CLI integrations remain outside the active runtime flow.
- Natural todo examples: `add todo buy milk`, `show my todos`, `complete 2`, `remember to pay rent`.
