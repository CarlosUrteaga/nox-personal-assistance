# nox-personal-assistance

Telegram-based personal assistant backed by Ollama.

## What it does

- Receives plain text messages in Telegram
- Sends them to an Ollama chat model
- Keeps a short in-memory conversation history per chat
- Stores internal todos locally in JSON
- Supports `/start`, `/help`, `/reset`, `/todo <task>`, `/todos`, and `/done <id>`
- Also handles common natural todo phrases before falling back to chat

## Requirements

- Rust toolchain
- Ollama running locally or remotely
- A Telegram bot token
- The numeric Telegram chat ID that should be allowed to talk to the bot

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

## Run

```bash
cargo run
```

## Notes

- Conversation memory is process-local and resets when the app restarts.
- Todos persist locally in the path configured by `TODO_STORE_PATH`.
- Google Workspace and `gemini` CLI integrations are currently not part of the active runtime flow.
- Natural todo examples: `add todo buy milk`, `show my todos`, `complete 2`, `remember to pay rent`.
