# Spotify

Control Spotify from KnobDeck, on Windows and macOS.

## What it does

Without an account or Premium (through the OS — Windows' media controls, Spotify's AppleScript
on macOS):

- Play / Pause (follows the player), Next Track, Previous Track.
- Now Playing: title and artist on a key; press to play or pause.

With your Spotify sign-in:

- Like Song (adds/removes the current track from Liked Songs).
- Shuffle, Repeat (off → all → one) and a Spotify Volume dial. *Premium*
- Play Playlist, a Playlists wheel (Liked Songs first) and Add Song to Playlist. *Play needs Premium*

Spotify only allows Premium accounts to control playback through its Web API, so the keys marked
*Premium* show "This needs Spotify Premium" on free accounts.

## Setup

Setup opens Spotify's sign-in page in your browser; approve KnobDeck and return. If you made your
own app at developer.spotify.com, paste its client ID in setup and add
`http://127.0.0.1:47811/callback` as its redirect URI.
