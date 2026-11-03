# Spotify

Control the Spotify desktop app from KnobDeck, on Windows and macOS. No sign-in, account or
Premium needed.

## What it does

- Play / Pause (follows the player), Next Track, Previous Track.
- Now Playing: title and artist on a key; press to play or pause.

## How it works

Through the OS, not keyboard shortcuts: Windows' system media controls (the session Spotify shows
in the volume flyout) and Spotify's AppleScript dictionary on macOS. Spotify must be open.

Likes, shuffle, repeat, volume and playlists through the Spotify Web API are written but switched
off for now (see `src/web.rs`); they need KnobDeck's Spotify app, whose owner must have Premium.
