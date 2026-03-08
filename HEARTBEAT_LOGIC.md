# Heartbeat Specification: Proactive Notification Logic

This specification defines the high-priority triggers for the NOX-Personal-Assistance "Heartbeat" service, which runs every 30 minutes to provide proactive Telegram notifications to the current user.

## 1. High-Priority Triggers (Proactive)

Based on the latest requirements, the heartbeat focuses on email and calendar synchronization:

### A. New Email Detection (General)
*   **Trigger:** Any new unread email in the inbox.
*   **Logic:** Summarize recent activity if new messages are detected.
*   **Action:** Send Telegram message with a brief summary of the sender and subject.

### B. Calendar Invitation & Schedule Sync
*   **Trigger:** New unread emails containing calendar invitations (ICS), meeting invites, or plain-text schedules/meeting details.
*   **Logic:** Sync invitations/schedules between priority emails defined in the `PRIORITY_EMAILS` environment variable.
    *   **External Sender:** If the sender is NOT in the priority list, create a calendar event with ALL priority emails as attendees.
    *   **Internal Sender:** If the sender IS one of the priority emails, create a calendar event with ONLY the missing email(s) in the list as attendees.
*   **Action:** Execute `calendar:create-event` with the target emails as attendees to trigger a formal invitation. Notify via Telegram with an "Invitation Sync Report".

## 2. Notification Format (Telegram)

Notifications should follow this concise format:
```text
🔔 NOX Heartbeat | [Category]
----------------------------
[Subject/Title]
Details: [Summary of actions taken or email content]
Time: [Relevant Time]
```

## 3. Deployment Constraints (Raspberry Pi)

*   **Frequency:** Every 30 minutes (Tokio Interval).
*   **Environment Variables:**
    *   `PRIORITY_EMAILS`: Comma-separated list (e.g., `a@mail.com,b@mail.com`).
    *   `TELOXIDE_TOKEN`: Telegram bot token.
    *   `CHAT_ID`: Your Telegram chat ID.
*   **State Management:** Maintain a `last_checked_id` for Gmail to avoid duplicate notifications for the same emails.
*   **Network:** Graceful handling of connection timeouts (90s timeout configured in Rust client).
