//! Clipboard history and setting the clipboard, through the desktop's own clipboard owner:
//! Klipper on Plasma (it keeps the history and owns the clipboard on Wayland), the KnobDeck
//! extension on GNOME (GNOME has no history of its own; the extension keeps the last 30 texts in
//! memory while the session runs).

use knobdeck_sdk::{json, Error, Result};
use linux_desktop::{bus, desktop, Desktop};

const KLIPPER: (&str, &str, &str) = ("org.kde.klipper", "/klipper", "org.kde.klipper.klipper");

/// Text entries, newest first.
pub fn history() -> Result<Vec<String>> {
    match desktop() {
        Desktop::Kde => bus::call(&bus::session()?, KLIPPER.0, KLIPPER.1, KLIPPER.2, "getClipboardHistoryMenu", &()).map_err(|_| Error::msg("Clipboard history needs Plasma's clipboard (Klipper) to be running")),
        Desktop::Gnome => Ok(linux_desktop::gnome::call("clipboard-history", &json!({}))?.as_array().map(|a| a.iter().filter_map(|v| v.as_str().map(String::from)).collect()).unwrap_or_default()),
        Desktop::Other => Err(linux_desktop::unsupported()),
    }
}

pub fn set_text(text: &str) -> Result {
    match desktop() {
        Desktop::Kde => bus::call0(&bus::session()?, KLIPPER.0, KLIPPER.1, KLIPPER.2, "setClipboardContents", &(text,)),
        Desktop::Gnome => linux_desktop::gnome::call("clipboard-set", &json!({ "text": text })).map(|_| ()),
        Desktop::Other => Err(linux_desktop::unsupported()),
    }
}

/// Paste into the focused app. Linux has no app-independent paste request (Wayland lets no
/// client act on another's text field), so this presses Ctrl+V, the paste chord GTK, Qt,
/// Electron and browser apps share. Terminals paste with Ctrl+Shift+V instead.
pub fn paste() -> Result {
    // Give the clipboard owner a moment to announce the new selection before the app reads it.
    std::thread::sleep(std::time::Duration::from_millis(60));
    knobdeck_sdk::send_keys("ctrl v")
}

/// A short one-line label for a wheel slice.
pub fn label(text: &str) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() > 40 {
        format!("{}…", line.chars().take(39).collect::<String>())
    } else {
        line
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn labels_are_single_line_and_short() {
        assert_eq!(super::label("a\n  b\tc"), "a b c");
        assert_eq!(super::label(&"x".repeat(50)).chars().count(), 40);
    }
}
