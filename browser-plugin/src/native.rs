//! The browser's own interface, used before any shortcut (see CLAUDE.md).
//!
//! macOS: Chrome, Edge and Brave (and other Chromium builds) publish an
//! AppleScript dictionary with tabs, navigation and windows, so those actions
//! go through it: no focus or keyboard layout involved, and a page can't
//! swallow the chord. It needs a one-time Automation permission ("KnobDeck wants
//! to control Google Chrome"); if it is refused or the call fails, the caller
//! falls back to the shortcut. Firefox and Zen publish no scripting interface.
//!
//! Windows: Chromium and Firefox expose no control interface to other apps
//! (the DevTools protocol only exists when the browser was started with a debug
//! flag), so the shortcut is the route there.

/// Run `action` through the browser's own interface. `true` = done; `false` =
/// not available for this browser/action (the caller sends the shortcut).
#[cfg(not(target_os = "macos"))]
pub fn try_run(_browser: &str, _action: &str) -> bool {
    false
}

#[cfg(target_os = "macos")]
pub use mac::try_run;

#[cfg(target_os = "macos")]
mod mac {
    use crate::browsers::{detect_key, exe_path, meta, Family};
    use knobdeck_sdk::{log, Value};
    use core_foundation::bundle::CFBundle;
    use core_foundation::string::CFString;
    use core_foundation::url::CFURL;

    /// AppleScript (inside `tell application id …`) for each action the
    /// Chromium dictionary covers. Everything else has no scripting command.
    fn chromium_script(action: &str) -> Option<&'static str> {
        Some(match action {
            "new-tab" => "if (count of windows) = 0 then\n make new window\nelse\n tell front window to make new tab\nend if",
            "new-window" => "make new window",
            "new-private-window" => "make new window with properties {mode:\"incognito\"}",
            "close-tab" => "close active tab of front window",
            "next-tab" => "tell front window to set active tab index to ((active tab index) mod (count of tabs)) + 1",
            "previous-tab" => "tell front window\n set i to (active tab index) - 1\n if i < 1 then set i to count of tabs\n set active tab index to i\nend tell",
            "reload-tab" => "reload active tab of front window",
            "go-back" => "go back active tab of front window",
            "go-forward" => "go forward active tab of front window",
            "stop-loading" => "stop active tab of front window",
            // chrome:// pages; Edge and Brave map them to their own schemes.
            "open-downloads" => "tell front window to make new tab with properties {URL:\"chrome://downloads\"}",
            "open-history" => "tell front window to make new tab with properties {URL:\"chrome://history\"}",
            "open-bookmarks-manager" => "tell front window to make new tab with properties {URL:\"chrome://bookmarks\"}",
            _ => return None,
        })
    }

    /// The app's bundle id from its Info.plist (handles Beta/Canary/Dev builds).
    fn bundle_id(app: &str) -> Option<String> {
        let url = CFURL::from_path(app, true)?;
        let bundle = CFBundle::new(url)?;
        let id = bundle.info_dictionary().find(CFString::from_static_string("CFBundleIdentifier"))?.downcast::<CFString>()?.to_string();
        // It is spliced into the script: allow bundle-id characters only.
        id.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-').then_some(id)
    }

    pub fn try_run(browser: &str, action: &str) -> bool {
        let key = detect_key(&Value::String(browser.into())).unwrap_or(if browser.is_empty() { "chrome" } else { browser });
        if meta(key).family != Family::Chromium {
            return false;
        }
        let Some(body) = chromium_script(action) else { return false };
        let Some(id) = exe_path(browser).as_deref().and_then(bundle_id) else { return false };
        let script = format!("tell application id \"{id}\"\n{body}\nend tell");
        match std::process::Command::new("/usr/bin/osascript").arg("-e").arg(&script).output() {
            Ok(out) if out.status.success() => true,
            Ok(out) => {
                log(format!("{action} via AppleScript failed, using the shortcut: {}", String::from_utf8_lossy(&out.stderr).trim()));
                false
            }
            Err(e) => {
                log(format!("osascript: {e}"));
                false
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn chrome_bundle_id_reads_from_info_plist() {
            if std::path::Path::new("/Applications/Google Chrome.app").exists() {
                assert_eq!(bundle_id("/Applications/Google Chrome.app").as_deref(), Some("com.google.Chrome"));
            }
            assert_eq!(bundle_id("/no/such.app"), None);
        }

        #[test]
        fn scripts_cover_navigation_but_not_page_shortcuts() {
            for a in ["new-tab", "close-tab", "next-tab", "previous-tab", "reload-tab", "go-back", "open-history"] {
                assert!(chromium_script(a).is_some(), "{a}");
            }
            for a in ["find-in-page", "print-page", "hard-reload-tab", "reopen-closed-tab", "toggle-fullscreen"] {
                assert!(chromium_script(a).is_none(), "{a}");
            }
        }
    }
}
