# YouTube

YouTube live streaming controls in KnobDeck, on Windows and macOS.

## What it does

- Viewers: concurrent viewers of your live stream (updated every minute), "Offline" otherwise.
- Chat Message: post a preset message in your live chat.
- Play Ad: insert an ad break (channels eligible for mid-roll ads).
- Live Dashboard: opens YouTube Studio's live streaming page.
- Go Live / End Stream: takes your next scheduled stream live, or ends the live one. YouTube needs
  your encoder (OBS…) to be sending video before a stream can go live.

## Setup

Setup opens Google's sign-in page; choose the account that owns your channel and allow KnobDeck.
If you use your own Google Cloud app (type Desktop, YouTube Data API enabled), enter its client ID
and secret in setup; they are kept only in KnobDeck's plugin data folder on this computer.

## How it works

The YouTube Data API with your Google sign-in. Every call counts against YouTube's daily API quota,
so the live keys refresh once a minute.

No client secret ships in the plugin. With KnobDeck's Google app, the code exchange and token
refresh go to the KnobDeck website (`KNOBDECK_GOOGLE_TOKEN_URL`, set at build time), which must:
accept the same form POST as `https://oauth2.googleapis.com/token` (`grant_type`, `code`,
`redirect_uri`, `code_verifier`, `client_id`, or `refresh_token`), add `client_secret`, forward it to
Google and return Google's JSON reply unchanged.
