# Claude

A dial/deck UI for the Claude Code sessions already running on this computer (Windows and macOS). It only **reads**
Claude's local state in `~/.claude` (or `CLAUDE_CONFIG_DIR`). It never calls a model, never
sends a prompt, and needs no account or setup. The only thing it ever types into a session is
the `/rc` command, which turns on Remote Control.

| Action | Type | What it does |
|---|---|---|
| Claude Sessions | wheel, live | The tile shows one dot per session: 🟠 running, 🔵 needs you (permission prompt or question), 🟢 done (idle), 🔴 interrupted, blocked (login, rate limit, API error) or stopped. It updates as soon as Claude Code writes its status. Each wheel slice says what the session is waiting for. Picking a session brings its terminal or app window to the front; picking a stopped one resumes it. |
| Remote Control | button, live | Context aware: acts on the session whose window is in front, else on the most recently active one. If Remote Control is off, it types `/rc`. If it is already on, it opens the session on claude.ai/code. |
| Remote Control… | wheel | The same, for any running session. |
| Usage | button, live | Tokens and messages over the last 5 h, 7 d or 30 d. Press it to cycle between them. |
| Recent Sessions | wheel | The last 5 sessions from `history.jsonl`. **Open in → Claude Code** resumes the session in a new terminal (`claude --resume`). **Claude app** resumes it with `--remote-control` and opens claude.ai/code. A session that is already running gets focused instead. |

## Where the data comes from

Live state is what Claude Code itself publishes. It is not a pipe protocol. A directory watcher
on `sessions/` pushes the tile the moment a session's file changes, and a 2 s poll covers
focus changes.

About the `messagingSocketPath` pipe (`\\.\pipe\LOCAL\cc-msg-…` on Windows, a unix socket on macOS): it is Claude Code's
cross-session **inbox**, authenticated with the per-session `.key` tokens. It accepts `user`
frames, which become messages in the conversation, plus a few control frames: `rename`,
`notify_when_idle` and artifact bookkeeping. It has no status query and no Remote Control
command, so this plugin does not connect to it. Sending prompts is exactly what this plugin
must not do. Everything the pipe could report also lands in `sessions/<pid>.json`.

- `sessions/<pid>.json`: written by each running Claude Code. It holds status (`busy` / `idle` / `waiting` + `waitingFor`), `state` / `needs` (blocked, failed),
  cwd, name and `bridgeSessionId` (present while Remote Control is on). A file whose process is
  gone, checked by pid plus creation time, shows as **stopped** for 6 h.
- Interrupted: the session is idle and the last message in its transcript is Claude Code's
  `[Request interrupted by user]` marker.
- Usage: the `message.usage` of assistant lines in `projects/**/*.jsonl`, deduplicated by message
  id. Input + output + cache tokens are summed. It is read incrementally on a background thread:
  ~3 s for the first pass over ~1 GB, then milliseconds.

## Limits

- **These are local token counts, not your plan's 5 h / weekly limit percentages.**
- **Windows:** several sessions in one Windows Terminal window can't be told apart by tab.
  Switching focuses the window, not the tab. Remote Control refuses to type into a shared window
  unless it is already in front, because you are then on the tab you mean.
- **macOS:** sessions in **Terminal** and **iTerm2** are matched to their exact tab by tty, so
  switching selects the right tab and Remote Control always types into the right session. The
  first switch asks to let KnobDeck control Terminal / iTerm2 (System Settings → Privacy & Security
  → Automation). Other hosts (VS Code, the Claude app) are focused per app; several sessions in
  one of them count as a shared window, as on Windows. Resuming opens a new Terminal.app window.
  Telling which session is in front uses the Accessibility permission KnobDeck already has.
- `/rc` typed while a session is busy is queued by Claude Code and runs after the turn.

## Build

```bash
knobdeck-plugin dev       # dist/claude/ → "Load Unpacked" in KnobDeck
knobdeck-plugin build && knobdeck-plugin package
cargo test -p claude                          # unit tests
cargo test -p claude -- --ignored --nocapture # smoke test against this machine's ~/.claude
```
