//! Basic playback without an account or Premium, through the OS: Windows' system media controls
//! (SMTC, the same session the volume flyout shows), Spotify's AppleScript dictionary on macOS, or
//! MPRIS on Linux (the D-Bus media interface the Spotify client exports, which GNOME's and
//! Plasma's media controls use too).

pub struct NowPlaying {
    pub title: String,
    pub artist: String,
    pub playing: bool,
}

pub use imp::*;

#[cfg(windows)]
mod imp {
    use super::NowPlaying;
    use knobdeck_sdk::{Error, Result};
    use windows::Media::Control::{GlobalSystemMediaTransportControlsSession as Session, GlobalSystemMediaTransportControlsSessionManager as Manager, GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status};

    /// Spotify's media session (desktop installer: "Spotify.exe"; Store app: "SpotifyAB.SpotifyMusic_…").
    fn session() -> Result<Session> {
        let werr = |e: windows::core::Error| Error::msg(format!("Media controls unavailable: {e}"));
        let manager = Manager::RequestAsync().and_then(|op| op.get()).map_err(werr)?;
        let sessions = manager.GetSessions().map_err(werr)?;
        sessions
            .into_iter()
            .find(|s| s.SourceAppUserModelId().is_ok_and(|id| id.to_string().to_ascii_lowercase().contains("spotify")))
            .ok_or_else(|| Error::msg("Spotify is not open"))
    }

    fn done(what: &str, r: windows::core::Result<windows::Foundation::IAsyncOperation<bool>>) -> Result<()> {
        match r.and_then(|op| op.get()) {
            Ok(true) => Ok(()),
            Ok(false) => Err(Error::msg(format!("Spotify ignored {what}"))),
            Err(e) => Err(Error::msg(format!("{what} failed: {e}"))),
        }
    }

    pub fn play_pause() -> Result<()> {
        done("play/pause", session()?.TryTogglePlayPauseAsync())
    }
    pub fn next() -> Result<()> {
        done("next", session()?.TrySkipNextAsync())
    }
    pub fn previous() -> Result<()> {
        done("previous", session()?.TrySkipPreviousAsync())
    }

    pub fn now_playing() -> Result<NowPlaying> {
        let s = session()?;
        let props = s.TryGetMediaPropertiesAsync().and_then(|op| op.get()).map_err(|e| Error::msg(format!("Track info unavailable: {e}")))?;
        let playing = s.GetPlaybackInfo().and_then(|i| i.PlaybackStatus()).is_ok_and(|st| st == Status::Playing);
        Ok(NowPlaying { title: props.Title().map(|t| t.to_string()).unwrap_or_default(), artist: props.Artist().map(|a| a.to_string()).unwrap_or_default(), playing })
    }

    /// Whether any media session exists (for tests: the API itself works).
    #[cfg(test)]
    pub fn session_count() -> usize {
        Manager::RequestAsync().and_then(|op| op.get()).and_then(|m| m.GetSessions()).map_or(0, |s| s.Size().unwrap_or(0) as usize)
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::NowPlaying;
    use knobdeck_sdk::{Error, Result};

    /// Run AppleScript against Spotify only if it is already running (never launch it).
    fn tell(script: &str) -> Result<String> {
        let full = format!("if application \"Spotify\" is running then\ntell application \"Spotify\"\n{script}\nend tell\nelse\nreturn \"__not_running__\"\nend if");
        let out = std::process::Command::new("osascript").args(["-e", &full]).output().map_err(|e| Error::msg(format!("osascript failed: {e}")))?;
        let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !out.status.success() {
            return Err(Error::msg(format!("Spotify did not respond: {}", String::from_utf8_lossy(&out.stderr).trim())));
        }
        if text == "__not_running__" {
            return Err(Error::msg("Spotify is not open"));
        }
        Ok(text)
    }

    pub fn play_pause() -> Result<()> {
        tell("playpause").map(|_| ())
    }
    pub fn next() -> Result<()> {
        tell("next track").map(|_| ())
    }
    pub fn previous() -> Result<()> {
        tell("previous track").map(|_| ())
    }

    pub fn now_playing() -> Result<NowPlaying> {
        // One line per field, so titles with commas survive.
        let t = tell("return (name of current track) & linefeed & (artist of current track) & linefeed & (player state as string)")?;
        let mut l = t.lines();
        Ok(NowPlaying { title: l.next().unwrap_or("").into(), artist: l.next().unwrap_or("").into(), playing: l.next() == Some("playing") })
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::NowPlaying;
    use knobdeck_sdk::{Error, Result};
    use linux_desktop::bus;
    use std::collections::HashMap;
    use zbus::zvariant::OwnedValue;

    const PATH: &str = "/org/mpris/MediaPlayer2";
    const PLAYER: &str = "org.mpris.MediaPlayer2.Player";

    /// Spotify's MPRIS name: `org.mpris.MediaPlayer2.spotify` (deb, Snap and Flatpak builds alike;
    /// a second instance adds `.instance<pid>`).
    fn player() -> Result<String> {
        let mut names = bus::session_names("org.mpris.MediaPlayer2.spotify");
        names.sort();
        names.into_iter().next().ok_or_else(|| Error::msg("Spotify is not open"))
    }

    fn call(method: &str) -> Result<()> {
        bus::call0(&bus::session()?, &player()?, PATH, PLAYER, method, &())
    }

    pub fn play_pause() -> Result<()> {
        call("PlayPause")
    }
    pub fn next() -> Result<()> {
        call("Next")
    }
    pub fn previous() -> Result<()> {
        call("Previous")
    }

    pub fn now_playing() -> Result<NowPlaying> {
        now_playing_of(&player()?)
    }

    fn now_playing_of(name: &str) -> Result<NowPlaying> {
        let (conn, name) = (bus::session()?, name);
        let meta: HashMap<String, OwnedValue> = bus::get(&conn, &name, PATH, PLAYER, "Metadata")?;
        let status: String = bus::get(&conn, &name, PATH, PLAYER, "PlaybackStatus")?;
        let text = |k: &str| meta.get(k).and_then(|v| v.try_clone().ok()).and_then(|v| String::try_from(v).ok()).unwrap_or_default();
        let artist = meta.get("xesam:artist").and_then(|v| v.try_clone().ok()).and_then(|v| Vec::<String>::try_from(v).ok()).map(|a| a.join(", ")).unwrap_or_default();
        Ok(NowPlaying { title: text("xesam:title"), artist, playing: status == "Playing" })
    }

    /// MPRIS players on the session bus (for tests: the bus itself works).
    #[cfg(test)]
    pub fn player_count() -> usize {
        bus::session_names("org.mpris.MediaPlayer2.").len()
    }

    /// Decodes any running player's metadata the way Spotify's is decoded (read-only).
    #[cfg(test)]
    pub fn any_player_now_playing() -> Option<(String, Result<NowPlaying>)> {
        let name = bus::session_names("org.mpris.MediaPlayer2.").into_iter().next()?;
        Some((name.clone(), now_playing_of(&name)))
    }
}
