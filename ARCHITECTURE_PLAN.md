# NOX Personal Assistance - Architectural Plan

## Status: COMPLETED (Phase 1-4)

The monolith has been successfully refactored into a modular architecture.

## 1. Goal
Refactor the current monolithic application into a modular architecture ("AGIUi" style) that separates:
- **Core Agent Logic**: The "brain" that makes decisions and orchestrates tasks.
- **Tools**: Capabilities (Email, Calendar, etc.) that can read/write data.
- **Interfaces (Channels)**: The "frontend" (Telegram, CLI, Web) that handles user interaction and presentation.

## 2. Implemented Structure

```
src/
├── main.rs            # Entry point (initializes config, agent, and starts channels)
├── config.rs          # Centralized configuration (Env vars, constants)
├── agent/             # The Core Logic
│   ├── mod.rs
│   └── core.rs        # Main Agent struct (orchestrator)
├── tools/             # Capabilities (Gemini CLI wrappers)
│   ├── mod.rs         # Defines ToolResponse and DataType
│   ├── gmail.rs       # Email scanning & sync logic
│   ├── calendar.rs    # Calendar fetching
│   └── gemini.rs      # Generic Gemini command execution helper
└── channels/          # User Interfaces
    ├── mod.rs
    └── telegram.rs    # Teloxide implementation with ToolResponse rendering
```

## 3. Key Abstractions Implemented

### A. The `Tool` Trait (Structs)
Tools return `ToolResponse` with structured data (`DataType`).
```rust
pub struct ToolResponse {
    pub content: String,
    pub data_type: DataType, // Text, Markdown, CalendarEvent, EmailSummary
}
```

### B. The `Channel` (TelegramChannel)
Decoupled from logic. It receives `ToolResponse` and decides how to render it (e.g., formatting email summaries or calendar events).

### C. The `Agent` (NoxAgent)
Orchestrates the heartbeat loop and handles commands (`calendar`, `email`), delegating to the appropriate tools.

## 4. Next Steps / Future "AGIUi" Features
- **Dynamic Tables**: If a tool returns a `Vec<Row>`, the Channel renders it as an ASCII table (CLI) or a formatted message (Telegram).
- **Interactive Elements**: If a tool needs confirmation, the Agent returns a `RequestConfirmation` type, and the Channel renders buttons (Telegram) or a Y/N prompt (CLI).
- **Multiple Channels**: Add a CLI or Web interface that uses the same `NoxAgent`.
