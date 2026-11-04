# AI Stats

Plan limits and token usage for AI coding tools on this computer (Windows and macOS): Claude Code
and Codex for now. It reads the login and transcripts each CLI already keeps. It never calls a
model, never refreshes or writes a token, and needs no setup.

| Action | Tile | Inputs |
|---|---|---|
| Claude Limits | Ring gauge in Claude orange: `Claude 5h 18% · 1h 48m` (used %, time to reset). Red from 90 %. Press to refetch. | **Credentials path** (default `~/.claude/.credentials.json`; on macOS the Keychain item `Claude Code-credentials` is used when the file is missing). **Usage type**: 5-hour, 7-day, Fable 7-day. |
| Codex Limits | Ring gauge in Codex blue: `Codex 5h 40% · 2h 5m`. The window name comes from the plan (Go has one 30-day window). | **Credentials path** (default `~/.codex/auth.json`). **Usage type**: primary (5-hour), secondary (weekly). |
| Claude Usage | Tokens used over the last 30 days: `Claude · 3.7B tokens · 30d`. | **Data folder** (default `~/.claude`). |
| Codex Usage | The same for Codex. | **Data folder** (default `~/.codex`). |

## Where the data comes from

- **Claude limits:** `GET https://api.anthropic.com/api/oauth/usage` with Claude Code's OAuth token
  (`five_hour`, `seven_day`, and the `weekly_scoped` Fable entry in `limits`).
- **Codex limits:** `GET https://chatgpt.com/backend-api/codex/usage` with the Codex CLI's token
  and account id (`rate_limit.primary_window` / `secondary_window`).
- Both are fetched at most once per 2 minutes per credentials file; tiles re-read the cache every 5 s.
- **Claude usage:** `message.usage` of assistant lines in `projects/**/*.jsonl`, deduplicated by
  message id; input + output + cache tokens.
- **Codex usage:** `token_count` events in `sessions/YYYY/MM/DD/*.jsonl`
  (`last_token_usage.total_tokens`, deduplicated by the session's cumulative total).
- Transcripts are read incrementally: the first pass over ~1 GB takes ~15 s, later ones milliseconds.

## Limits

- Tokens are not refreshed: refreshing would rotate the CLI's refresh token and sign it out. When
  the stored token has expired the tile says **sign in** until Claude Code / Codex is used again
  (they refresh it themselves).
- `CLAUDE_CONFIG_DIR` / `CODEX_HOME` are not read; set the path inputs instead.
- Both usage endpoints are undocumented and may change.

## Build

```bash
knobdeck-plugin dev       # dist/ai-stats-<os>/ → "Load Unpacked" in KnobDeck
knobdeck-plugin build && knobdeck-plugin package
cargo test -p ai-stats                           # unit tests
cargo test -p ai-stats -- --ignored --nocapture  # smoke test against this machine's logins and transcripts
```
