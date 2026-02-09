---
name: macos-automation
description: macOS automation via osascript for Finder, Calendar, Reminders, Notifications, and system control
category: macos
requires:
  - exec
---

# macOS Automation

Use `exec:run` with `program: "osascript"` to automate macOS applications and system functions via AppleScript. Use `program: "open"` for simple launch/URL tasks.

## Prerequisites

- **macOS only** — AppleScript is not available on other platforms.
- **Automation permission** — macOS prompts for permission the first time `osascript` controls each application (System Settings → Privacy & Security → Automation).
- **Accessibility permission** — required for UI scripting via System Events (System Settings → Privacy & Security → Accessibility).
- `osascript` and `open` must be in the exec allowlist.

## Calling Convention

Single-line AppleScript:

```json
{ "program": "osascript", "args": ["-e", "display notification \"Hello\" with title \"Abbot\""] }
```

Multi-line AppleScript (use `\n` within the string):

```json
{ "program": "osascript", "args": ["-e", "tell application \"Finder\"\nset fileCount to count of items of (path to desktop)\nreturn fileCount\nend tell"] }
```

JavaScript for Automation (JXA) with `-l JavaScript`:

```json
{ "program": "osascript", "args": ["-l", "JavaScript", "-e", "Application('System Events').processes().map(p => p.name())"] }
```

---

## Notifications

### Display a notification

```json
{ "program": "osascript", "args": ["-e", "display notification \"Task completed successfully\" with title \"Abbot\" subtitle \"Build Pipeline\""] }
```

### Notification with sound

```json
{ "program": "osascript", "args": ["-e", "display notification \"Tests passed\" with title \"Abbot\" sound name \"Glass\""] }
```

Common sound names: `Glass`, `Ping`, `Pop`, `Purr`, `Sosumi`, `Submarine`, `Tink`.

---

## Dialogs & User Interaction

### Display an alert

```json
{ "program": "osascript", "args": ["-e", "display alert \"Build Complete\" message \"All 42 tests passed.\" as informational"] }
```

Alert types: `informational`, `warning`, `critical`.

### Yes/No dialog

```json
{ "program": "osascript", "args": ["-e", "display dialog \"Deploy to production?\" buttons {\"Cancel\", \"Deploy\"} default button \"Cancel\""] }
```

Returns `button returned:Deploy` or errors on Cancel.

### Text input dialog

```json
{ "program": "osascript", "args": ["-e", "display dialog \"Enter branch name:\" default answer \"feature/\""] }
```

Returns `button returned:OK, text returned:feature/my-branch`.

### Choose from list

```json
{ "program": "osascript", "args": ["-e", "choose from list {\"Development\", \"Staging\", \"Production\"} with prompt \"Select environment:\" default items {\"Development\"}"] }
```

---

## Finder

### Open a folder in Finder

```json
{ "program": "open", "args": ["/Users/me/projects"] }
```

### Reveal a file in Finder

```json
{ "program": "open", "args": ["-R", "/Users/me/projects/app/src/main.rs"] }
```

### Get selected files in Finder

```json
{ "program": "osascript", "args": ["-e", "tell application \"Finder\" to get POSIX path of (selection as alias list)"] }
```

### Get current Finder directory

```json
{ "program": "osascript", "args": ["-e", "tell application \"Finder\" to get POSIX path of (target of front window as alias)"] }
```

### Create a new folder

```json
{ "program": "osascript", "args": ["-e", "tell application \"Finder\" to make new folder at desktop with properties {name:\"New Project\"}"] }
```

### Move file to Trash

```json
{ "program": "osascript", "args": ["-e", "tell application \"Finder\" to delete POSIX file \"/Users/me/Desktop/old-file.txt\""] }
```

### Empty Trash

```json
{ "program": "osascript", "args": ["-e", "tell application \"Finder\" to empty trash"] }
```

### Get disk space

```json
{ "program": "osascript", "args": ["-e", "tell application \"Finder\"\nset diskInfo to properties of startup disk\nreturn {free space:free space of diskInfo, capacity:capacity of diskInfo}\nend tell"] }
```

---

## Calendar

### List calendars

```json
{ "program": "osascript", "args": ["-e", "tell application \"Calendar\" to get name of every calendar"] }
```

### List today's events

```json
{ "program": "osascript", "args": ["-e", "set today to current date\nset hours of today to 0\nset minutes of today to 0\nset seconds of today to 0\nset tomorrow to today + (1 * days)\ntell application \"Calendar\"\nset output to \"\"\nrepeat with cal in calendars\nset evts to (every event of cal whose start date ≥ today and start date < tomorrow)\nrepeat with evt in evts\nset output to output & (summary of evt) & \" @ \" & (start date of evt as string) & linefeed\nend repeat\nend repeat\nreturn output\nend tell"] }
```

### Create an event

```json
{ "program": "osascript", "args": ["-e", "tell application \"Calendar\"\ntell calendar \"Work\"\nset startDate to current date\nset hours of startDate to 14\nset minutes of startDate to 0\nset seconds of startDate to 0\nset endDate to startDate + (1 * hours)\nmake new event with properties {summary:\"Team standup\", start date:startDate, end date:endDate, location:\"Zoom\"}\nend tell\nend tell"] }
```

### Create an all-day event

```json
{ "program": "osascript", "args": ["-e", "tell application \"Calendar\"\ntell calendar \"Personal\"\nset eventDate to current date\nset hours of eventDate to 0\nset minutes of eventDate to 0\nset seconds of eventDate to 0\nmake new event with properties {summary:\"Vacation\", start date:eventDate, allday event:true}\nend tell\nend tell"] }
```

### Delete an event by name (today)

```json
{ "program": "osascript", "args": ["-e", "set today to current date\nset hours of today to 0\nset minutes of today to 0\nset seconds of today to 0\nset tomorrow to today + (1 * days)\ntell application \"Calendar\"\nrepeat with cal in calendars\nset evts to (every event of cal whose summary is \"Team standup\" and start date ≥ today and start date < tomorrow)\nrepeat with evt in evts\ndelete evt\nend repeat\nend repeat\nend tell"] }
```

---

## Reminders

### List reminder lists

```json
{ "program": "osascript", "args": ["-e", "tell application \"Reminders\" to get name of every list"] }
```

### List incomplete reminders

```json
{ "program": "osascript", "args": ["-e", "tell application \"Reminders\"\nset output to \"\"\nrepeat with rem in (reminders whose completed is false)\nset output to output & (name of rem) & linefeed\nend repeat\nreturn output\nend tell"] }
```

### List reminders in a specific list

```json
{ "program": "osascript", "args": ["-e", "tell application \"Reminders\"\nset output to \"\"\nrepeat with rem in (reminders of list \"Work\" whose completed is false)\nset output to output & (name of rem) & linefeed\nend repeat\nreturn output\nend tell"] }
```

### Create a reminder

```json
{ "program": "osascript", "args": ["-e", "tell application \"Reminders\"\ntell list \"Work\"\nmake new reminder with properties {name:\"Review pull request #42\", body:\"Check the auth refactor branch\"}\nend tell\nend tell"] }
```

### Create a reminder with a due date

```json
{ "program": "osascript", "args": ["-e", "set dueDate to current date\nset hours of dueDate to 17\nset minutes of dueDate to 0\ntell application \"Reminders\"\ntell list \"Work\"\nmake new reminder with properties {name:\"Submit report\", due date:dueDate}\nend tell\nend tell"] }
```

### Complete a reminder

```json
{ "program": "osascript", "args": ["-e", "tell application \"Reminders\"\nrepeat with rem in (reminders whose name is \"Review pull request #42\" and completed is false)\nset completed of rem to true\nend repeat\nend tell"] }
```

### Delete a reminder

```json
{ "program": "osascript", "args": ["-e", "tell application \"Reminders\"\ndelete (first reminder whose name is \"Old task\")\nend tell"] }
```

---

## Notes

### List note folders

```json
{ "program": "osascript", "args": ["-e", "tell application \"Notes\" to get name of every folder"] }
```

### List notes in a folder

```json
{ "program": "osascript", "args": ["-e", "tell application \"Notes\"\nset output to \"\"\nrepeat with n in notes of folder \"Notes\"\nset output to output & (name of n) & linefeed\nend repeat\nreturn output\nend tell"] }
```

### Create a note

```json
{ "program": "osascript", "args": ["-e", "tell application \"Notes\"\ntell folder \"Notes\"\nmake new note with properties {name:\"Meeting Notes\", body:\"<h1>Meeting Notes</h1><p>Discussed the roadmap.</p>\"}\nend tell\nend tell"] }
```

Note: The `body` field accepts HTML.

### Read a note's content

```json
{ "program": "osascript", "args": ["-e", "tell application \"Notes\"\nset n to first note whose name is \"Meeting Notes\"\nreturn body of n\nend tell"] }
```

---

## System Information & Control

### Get current date and time

```json
{ "program": "osascript", "args": ["-e", "return (current date) as string"] }
```

### Get system info

```json
{ "program": "osascript", "args": ["-e", "return system info"] }
```

### Get screen dimensions

```json
{ "program": "osascript", "args": ["-e", "tell application \"Finder\" to get bounds of window of desktop"] }
```

### Get current user

```json
{ "program": "osascript", "args": ["-e", "return short user name of (system info)"] }
```

### Get battery status

```json
{ "program": "osascript", "args": ["-l", "JavaScript", "-e", "ObjC.import('IOKit'); var snapshot = $.IOPSCopyPowerSourcesInfo(); var sources = $.IOPSCopyPowerSourcesList(snapshot); var info = $.IOPSGetPowerSourceDescription(snapshot, $.CFArrayGetValueAtIndex(sources, 0)); $.CFDictionaryGetValue(info, 'Current Capacity')"] }
```

### Set system volume

```json
{ "program": "osascript", "args": ["-e", "set volume output volume 50"] }
```

Volume range: 0–100.

### Mute/unmute

```json
{ "program": "osascript", "args": ["-e", "set volume with output muted"] }
{ "program": "osascript", "args": ["-e", "set volume without output muted"] }
```

### Get current volume

```json
{ "program": "osascript", "args": ["-e", "return output volume of (get volume settings)"] }
```

---

## Application Management

### List running applications

```json
{ "program": "osascript", "args": ["-e", "tell application \"System Events\" to get name of every process whose background only is false"] }
```

### Check if an application is running

```json
{ "program": "osascript", "args": ["-e", "tell application \"System Events\" to (name of processes) contains \"Safari\""] }
```

### Launch an application

```json
{ "program": "osascript", "args": ["-e", "launch application \"Safari\""] }
```

### Activate (bring to front)

```json
{ "program": "osascript", "args": ["-e", "tell application \"Safari\" to activate"] }
```

### Quit an application

```json
{ "program": "osascript", "args": ["-e", "tell application \"Safari\" to quit"] }
```

### Force quit an application

```json
{ "program": "osascript", "args": ["-e", "tell application \"Safari\" to quit saving no"] }
```

### List windows of an application

```json
{ "program": "osascript", "args": ["-e", "tell application \"System Events\" to get name of every window of process \"Safari\""] }
```

---

## URLs & Web

### Open a URL in default browser

```json
{ "program": "open", "args": ["https://github.com"] }
```

### Open a URL in a specific browser

```json
{ "program": "open", "args": ["-a", "Safari", "https://github.com"] }
```

### Open a file with a specific application

```json
{ "program": "open", "args": ["-a", "Visual Studio Code", "/Users/me/projects/app"] }
```

---

## Clipboard

### Get clipboard contents

```json
{ "program": "osascript", "args": ["-e", "return the clipboard"] }
```

### Set clipboard contents

```json
{ "program": "osascript", "args": ["-e", "set the clipboard to \"Hello from Abbot\""] }
```

---

## Terminal / Shell Integration

### Run a command in Terminal.app

```json
{ "program": "osascript", "args": ["-e", "tell application \"Terminal\"\nactivate\ndo script \"cd ~/projects && cargo test\"\nend tell"] }
```

### Open a new Terminal tab

```json
{ "program": "osascript", "args": ["-e", "tell application \"Terminal\"\nactivate\ntell application \"System Events\" to keystroke \"t\" using command down\nend tell"] }
```

Note: UI scripting (keystroke) requires Accessibility permission.

---

## Common Workflows

### Notify on task completion

After a long-running build or test:

```json
{ "program": "osascript", "args": ["-e", "display notification \"Build succeeded — 142 tests passed\" with title \"Abbot\" sound name \"Glass\""] }
```

### Daily standup prep

1. List today's calendar events
2. List incomplete reminders from "Work" list
3. Compile into a summary

### Quick note capture

1. Get clipboard contents (if relevant)
2. Create a note in Notes.app with timestamp and context

### Pre-meeting setup

1. Open relevant URLs in browser
2. Open project in VS Code
3. Display notification that setup is ready

---

## Safety Notes

- **Read-only** (safe for hand agents): Getting clipboard, listing applications/windows/calendars/reminders/notes, checking if apps are running, system info, volume settings, Finder selection, reading notes
- **Mutating** (requires head/mind): Sending notifications, creating/deleting calendar events, creating/completing/deleting reminders, creating notes, setting clipboard, setting volume, launching/quitting applications, Finder operations (move to trash, create folder, empty trash), opening URLs, Terminal commands
- **Dialogs block execution** — `display dialog`, `display alert`, and `choose from list` wait for user interaction. Use `display notification` for non-blocking alerts.
- **Permissions accumulate** — each new application controlled by `osascript` triggers a separate macOS permission prompt the first time.
- **UI scripting is fragile** — keystroke-based automation (via System Events) depends on UI state and may break across macOS versions. Prefer direct AppleScript commands when available.
- `open` is in the default exec allowlist. `osascript` must also be in the allowlist.
