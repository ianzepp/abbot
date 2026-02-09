---
name: macos-imessage
description: Send and read iMessages on macOS via osascript and sqlite3
category: macos
requires:
  - exec
---

# iMessage on macOS

Use `exec:run` with `program: "osascript"` to send messages and control Messages.app, and `program: "sqlite3"` to read message history from the local database.

## Prerequisites

- **macOS only** — iMessage APIs are not available on other platforms.
- **Messages.app** must be signed in to an iMessage account.
- **Full Disk Access** must be granted to the process reading `chat.db` (System Settings → Privacy & Security → Full Disk Access).
- **Automation permission** must be granted for `osascript` to control Messages.app (macOS prompts on first use).
- `osascript` must be in the exec allowlist.

---

## Launching & Managing Messages.app

### Check if Messages is running

```json
{ "program": "osascript", "args": ["-e", "tell application \"System Events\" to (name of processes) contains \"Messages\""] }
```

Returns `true` or `false`.

### Launch Messages (background, no focus steal)

```json
{ "program": "osascript", "args": ["-e", "launch application \"Messages\""] }
```

### Launch Messages and bring to foreground

```json
{ "program": "osascript", "args": ["-e", "tell application \"Messages\" to activate"] }
```

### Quit Messages

```json
{ "program": "osascript", "args": ["-e", "tell application \"Messages\" to quit"] }
```

### Launch and wait for readiness

```json
{ "program": "osascript", "args": ["-e", "launch application \"Messages\"\ndelay 3\ntell application \"Messages\" to count of services"] }
```

If the result is a number greater than 0, Messages is ready to send.

---

## Sending Messages

### Send to a phone number

```json
{ "program": "osascript", "args": ["-e", "tell application \"Messages\"\nset targetService to 1st service whose service type = iMessage\nset targetBuddy to buddy \"+15551234567\" of targetService\nsend \"Hello from Abbot\" to targetBuddy\nend tell"] }
```

### Send to an email address

```json
{ "program": "osascript", "args": ["-e", "tell application \"Messages\"\nset targetService to 1st service whose service type = iMessage\nset targetBuddy to buddy \"user@example.com\" of targetService\nsend \"Hello from Abbot\" to targetBuddy\nend tell"] }
```

### Send with launch guard

Ensures Messages is running before sending:

```json
{ "program": "osascript", "args": ["-e", "if application \"Messages\" is not running then\nlaunch application \"Messages\"\ndelay 3\nend if\ntell application \"Messages\"\nset targetService to 1st service whose service type = iMessage\nset targetBuddy to buddy \"+15551234567\" of targetService\nsend \"Hello from Abbot\" to targetBuddy\nend tell"] }
```

### Send to a group chat by name

```json
{ "program": "osascript", "args": ["-e", "tell application \"Messages\"\nsend \"Hello everyone\" to chat \"Family Chat\"\nend tell"] }
```

Note: The chat name must match exactly as shown in Messages.app.

### Send a multi-line message

```json
{ "program": "osascript", "args": ["-e", "tell application \"Messages\"\nset targetService to 1st service whose service type = iMessage\nset targetBuddy to buddy \"+15551234567\" of targetService\nset msg to \"Line one\" & linefeed & \"Line two\" & linefeed & \"Line three\"\nsend msg to targetBuddy\nend tell"] }
```

### List available services

```json
{ "program": "osascript", "args": ["-e", "tell application \"Messages\" to get name of every service"] }
```

### List available buddies on iMessage service

```json
{ "program": "osascript", "args": ["-e", "tell application \"Messages\"\nset targetService to 1st service whose service type = iMessage\nget id of every buddy of targetService\nend tell"] }
```

---

## Reading Message History (SQLite)

Message history is stored at `~/Library/Messages/chat.db`. Requires Full Disk Access.

### Date conversion note

Apple stores dates as **nanoseconds since 2001-01-01**. To convert to a readable datetime:

```sql
datetime(date/1000000000 + 978307200, 'unixepoch', 'localtime')
```

### Read recent messages

```json
{ "program": "sqlite3", "args": ["-header", "-column", "~/Library/Messages/chat.db", "SELECT datetime(m.date/1000000000 + 978307200, 'unixepoch', 'localtime') as time, h.id as sender, m.is_from_me, substr(m.text, 1, 80) as text FROM message m LEFT JOIN handle h ON m.handle_id = h.ROWID ORDER BY m.date DESC LIMIT 20;"] }
```

### Read messages from a specific contact

```json
{ "program": "sqlite3", "args": ["-header", "-column", "~/Library/Messages/chat.db", "SELECT datetime(m.date/1000000000 + 978307200, 'unixepoch', 'localtime') as time, m.is_from_me, m.text FROM message m JOIN handle h ON m.handle_id = h.ROWID WHERE h.id = '+15551234567' ORDER BY m.date DESC LIMIT 20;"] }
```

### Read messages from a specific chat/conversation

```json
{ "program": "sqlite3", "args": ["-header", "-column", "~/Library/Messages/chat.db", "SELECT datetime(m.date/1000000000 + 978307200, 'unixepoch', 'localtime') as time, h.id as sender, m.is_from_me, m.text FROM message m JOIN chat_message_join cmj ON m.ROWID = cmj.message_id JOIN chat c ON cmj.chat_id = c.ROWID LEFT JOIN handle h ON m.handle_id = h.ROWID WHERE c.display_name = 'Family Chat' ORDER BY m.date DESC LIMIT 20;"] }
```

### List all conversations

```json
{ "program": "sqlite3", "args": ["-header", "-column", "~/Library/Messages/chat.db", "SELECT c.chat_identifier, c.display_name, c.service_name, datetime(c.last_read_message_timestamp/1000000000 + 978307200, 'unixepoch', 'localtime') as last_read FROM chat c ORDER BY c.last_read_message_timestamp DESC LIMIT 20;"] }
```

### List all known contacts/handles

```json
{ "program": "sqlite3", "args": ["-header", "-column", "~/Library/Messages/chat.db", "SELECT id, service FROM handle ORDER BY id;"] }
```

### Count unread messages

```json
{ "program": "sqlite3", "args": ["~/Library/Messages/chat.db", "SELECT COUNT(*) FROM message WHERE is_read = 0 AND is_from_me = 0;"] }
```

### Messages since a specific time

```json
{ "program": "sqlite3", "args": ["-header", "-column", "~/Library/Messages/chat.db", "SELECT datetime(m.date/1000000000 + 978307200, 'unixepoch', 'localtime') as time, h.id as sender, m.is_from_me, m.text FROM message m LEFT JOIN handle h ON m.handle_id = h.ROWID WHERE m.date > (strftime('%s', '2025-01-15') - 978307200) * 1000000000 ORDER BY m.date DESC LIMIT 50;"] }
```

### Search messages by text

```json
{ "program": "sqlite3", "args": ["-header", "-column", "~/Library/Messages/chat.db", "SELECT datetime(m.date/1000000000 + 978307200, 'unixepoch', 'localtime') as time, h.id as sender, m.text FROM message m LEFT JOIN handle h ON m.handle_id = h.ROWID WHERE m.text LIKE '%meeting%' ORDER BY m.date DESC LIMIT 20;"] }
```

### List attachments for recent messages

```json
{ "program": "sqlite3", "args": ["-header", "-column", "~/Library/Messages/chat.db", "SELECT datetime(m.date/1000000000 + 978307200, 'unixepoch', 'localtime') as time, a.filename, a.mime_type, a.total_bytes FROM message m JOIN message_attachment_join maj ON m.ROWID = maj.message_id JOIN attachment a ON maj.attachment_id = a.ROWID ORDER BY m.date DESC LIMIT 20;"] }
```

### Group chat members

```json
{ "program": "sqlite3", "args": ["-header", "-column", "~/Library/Messages/chat.db", "SELECT c.display_name, h.id as member FROM chat c JOIN chat_handle_join chj ON c.ROWID = chj.chat_id JOIN handle h ON chj.handle_id = h.ROWID WHERE c.display_name = 'Family Chat';"] }
```

---

## Monitoring for New Messages

Poll the database for messages newer than the last-seen sequence. The `ROWID` column on the `message` table is auto-incrementing:

### Get the latest message ROWID

```json
{ "program": "sqlite3", "args": ["~/Library/Messages/chat.db", "SELECT MAX(ROWID) FROM message;"] }
```

### Poll for messages newer than a known ROWID

```json
{ "program": "sqlite3", "args": ["-header", "-column", "~/Library/Messages/chat.db", "SELECT m.ROWID, datetime(m.date/1000000000 + 978307200, 'unixepoch', 'localtime') as time, h.id as sender, m.is_from_me, m.text FROM message m LEFT JOIN handle h ON m.handle_id = h.ROWID WHERE m.ROWID > 12345 ORDER BY m.ROWID ASC;"] }
```

Replace `12345` with the last-seen ROWID. Poll on an interval (e.g., every 30 seconds) to detect new messages.

---

## Database Schema Reference

Key tables in `~/Library/Messages/chat.db`:

| Table | Purpose | Key Columns |
|-------|---------|-------------|
| `message` | Individual messages | `ROWID`, `text`, `handle_id`, `date`, `is_from_me`, `is_read`, `cache_has_attachments` |
| `handle` | Contacts | `ROWID`, `id` (phone/email), `service` |
| `chat` | Conversations | `ROWID`, `chat_identifier`, `display_name`, `service_name` |
| `chat_message_join` | Links messages ↔ chats | `chat_id`, `message_id` |
| `chat_handle_join` | Links handles ↔ chats | `chat_id`, `handle_id` |
| `attachment` | File attachments | `ROWID`, `filename`, `mime_type`, `total_bytes` |
| `message_attachment_join` | Links messages ↔ attachments | `message_id`, `attachment_id` |

---

## Safety Notes

- **Read-only** (safe for hand agents): All `sqlite3` queries against `chat.db`, listing services/buddies, checking if Messages is running
- **Mutating** (requires head/mind): Sending messages, launching/quitting Messages.app
- **Never send messages without explicit user intent** — sending to the wrong contact or with wrong content cannot be undone.
- The `chat.db` database is read-only via `sqlite3` — you cannot insert or modify messages through SQLite, only through Messages.app via AppleScript.
- Phone numbers must include country code (e.g., `+15551234567`, not `5551234567`).
- The `service type` matters: `iMessage` for Apple devices, `SMS` for carrier SMS. Using the wrong one silently fails.
- Group chats must be referenced by their exact display name or `chat_identifier`.
- Full Disk Access is a system-level permission — the user must grant it manually in System Settings.
