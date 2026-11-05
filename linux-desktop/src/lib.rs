//! Shared by the Linux plugins (`linux-tools`, `browser-plugin`): which desktop is running, a
//! window bridge for KDE Plasma (KWin scripting) and GNOME (the KnobDeck Shell extension) that
//! works on Wayland and X11 alike, D-Bus helpers, and XDG desktop entries / icons.
//!
//! Supported desktops: **KDE Plasma 6** and **GNOME 45+**. Others get a clear error from the
//! window functions; the freedesktop parts (D-Bus, desktop entries) work everywhere.
#![cfg(target_os = "linux")]

pub mod apps;
pub mod bus;
pub mod gnome;
mod kwin;
pub mod wm;

/// The desktop environments the Linux plugins support.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Desktop {
    Kde,
    Gnome,
    Other,
}

/// The running desktop: the compositor that owns its D-Bus name, else `XDG_CURRENT_DESKTOP`
/// (KnobDeck and its plugins inherit the session's environment).
pub fn desktop() -> Desktop {
    if bus::session_has_owner("org.kde.KWin") {
        return Desktop::Kde;
    }
    if bus::session_has_owner("org.gnome.Shell") {
        return Desktop::Gnome;
    }
    desktop_from_env(&std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default())
}

fn desktop_from_env(v: &str) -> Desktop {
    // `KDE`, `GNOME`, `ubuntu:GNOME`, `Pop:GNOME`, …
    let parts: Vec<String> = v.split(':').map(|p| p.trim().to_ascii_lowercase()).collect();
    if parts.iter().any(|p| p == "kde") {
        Desktop::Kde
    } else if parts.iter().any(|p| p == "gnome") {
        Desktop::Gnome
    } else {
        Desktop::Other
    }
}

/// Error for features that need GNOME or KDE Plasma.
pub fn unsupported() -> knobdeck_sdk::Error {
    knobdeck_sdk::Error::msg("This action needs GNOME or KDE Plasma")
}

/// Start a program detached from the plugin (own process group, no inherited stdio), reaping it
/// in the background so it never lingers as a zombie.
pub fn spawn_detached(program: &str, args: &[&str]) -> knobdeck_sdk::Result {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .map_err(|e| knobdeck_sdk::Error::msg(format!("Could not start {program}: {e}")))?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// The first of `programs` found on `PATH`.
pub fn which(programs: &[&'static str]) -> Option<&'static str> {
    let path = std::env::var_os("PATH")?;
    programs.iter().copied().find(|p| std::env::split_paths(&path).any(|d| is_executable(&d.join(p))))
}

fn is_executable(p: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktop_env_parsing() {
        assert_eq!(desktop_from_env("KDE"), Desktop::Kde);
        assert_eq!(desktop_from_env("ubuntu:GNOME"), Desktop::Gnome);
        assert_eq!(desktop_from_env("GNOME-Classic:GNOME"), Desktop::Gnome);
        assert_eq!(desktop_from_env("Hyprland"), Desktop::Other);
        assert_eq!(desktop_from_env(""), Desktop::Other);
    }

    #[test]
    fn which_finds_sh() {
        assert_eq!(which(&["no-such-program-kd", "sh"]), Some("sh"));
    }
}
