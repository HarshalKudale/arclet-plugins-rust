//! claude — a dial/deck UI for the Claude Code sessions already running on this computer
//! (Windows and macOS).
//!
//! Read-only over Claude's local state (`~/.claude`): it never calls a model and never
//! sends prompts. The only input it ever types into a session is the `/rc` command that
//! turns Remote Control on.

#[cfg(not(any(windows, target_os = "macos")))]
fn main() {
    eprintln!("the claude plugin runs on Windows and macOS");
    std::process::exit(1);
}

/// OS backend: process tree, session windows, focus, terminals.
#[cfg(windows)]
#[path = "win.rs"]
mod os;
#[cfg(target_os = "macos")]
#[path = "mac.rs"]
mod os;

#[cfg(any(windows, target_os = "macos"))]
mod sessions;
#[cfg(any(windows, target_os = "macos"))]
mod usage;

#[cfg(any(windows, target_os = "macos"))]
fn main() {
    app::run()
}

#[cfg(any(windows, target_os = "macos"))]
mod app {
    use crate::sessions::{self, folder_name, Session, Status};
    use crate::{os, usage};
    use knobdeck_sdk::input::{self, InputExt};
    use knobdeck_sdk::{json, log, Action, Ctx, Plugin, Result, Value, Visual};
    use base64::Engine;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use std::time::Duration;

    const LIVE: Duration = Duration::from_secs(2);

    fn svg_uri(svg: String) -> String {
        format!("data:image/svg+xml;base64,{}", base64::engine::general_purpose::STANDARD.encode(svg))
    }

    /// One status dot per session (up to 9, 3×3), or a grey ring when none run.
    fn dots_icon(statuses: &[Status]) -> String {
        if statuses.is_empty() {
            return svg_uri(r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><circle cx="12" cy="12" r="8" fill="none" stroke="#9ca3af" stroke-width="2"/></svg>"##.into());
        }
        let n = statuses.len().min(9);
        let cols = if n == 1 { 1 } else if n <= 4 { 2 } else { 3 };
        let cell = 24.0 / cols as f64;
        let r = cell * 0.36;
        let dots: String = statuses[..n]
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let (cx, cy) = (cell * ((i % cols) as f64 + 0.5), cell * ((i / cols) as f64 + 0.5));
                format!(r#"<circle cx="{cx:.1}" cy="{cy:.1}" r="{r:.1}" fill="{}"/>"#, s.color())
            })
            .collect();
        svg_uri(format!(r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">{dots}</svg>"#))
    }

    fn live_sessions() -> Vec<Session> {
        sessions::sessions().into_iter().filter(Session::alive).collect()
    }

    fn find_session(id: &str) -> Option<Session> {
        sessions::sessions().into_iter().find(|s| s.id == id)
    }

    fn open_url(url: &str) -> Result {
        os::open_url(url)?;
        Ok(())
    }

    fn focus(ctx: &Ctx, s: &Session) -> Result {
        match &s.window {
            Some(w) => {
                os::focus(w);
                Ok(())
            }
            None => ctx.fail(format!("Can't find the window hosting \"{}\"", s.name)),
        }
    }

    /// Turn Remote Control on by typing `/rc` into the session — or, if it is
    /// already on, open the session on claude.ai/code.
    fn remote_control(ctx: &Ctx, s: &Session) -> Result {
        if let Some(b) = &s.bridge {
            return open_url(&format!("https://claude.ai/code/{b}"));
        }
        if !s.alive() {
            return ctx.fail(format!("\"{}\" is not running", s.name));
        }
        let Some(w) = &s.window else { return ctx.fail(format!("Can't find the window hosting \"{}\"", s.name)) };
        if !os::front().shows(w) {
            // Typing into a shared window would hit whichever tab is active, maybe another session.
            if s.shared_window {
                return ctx.fail(format!("Several sessions share this window — switch to \"{}\"'s tab, then press Remote Control", s.name));
            }
            os::focus(w);
            std::thread::sleep(Duration::from_millis(350));
            if !os::front().shows(w) {
                return ctx.fail("Couldn't bring the session's window to the front");
            }
        }
        // Typed on purpose (CLAUDE.md allows keystrokes for text entry): Claude Code's
        // only connection point, the messaging socket, accepts user messages but has no
        // Remote Control command, and terminal scripting (Terminal `do script`, iTerm
        // `write text`) sends text + Return in one burst that Claude Code can take as a
        // paste instead of picking `/rc` from its command menu.
        for k in ["slash", "r", "c"] {
            ctx.send_keys(k)?;
        }
        std::thread::sleep(Duration::from_millis(200)); // let the command menu settle
        ctx.send_keys("enter")
    }

    fn valid_id(id: &str) -> bool {
        !id.is_empty() && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
    }

    /// Resume a past session in a new terminal (Windows Terminal / a console on
    /// Windows, Terminal.app on macOS).
    fn resume_in_terminal(ctx: &Ctx, id: &str, cwd: &str, remote: bool) -> Result {
        if !valid_id(id) {
            return ctx.fail("Invalid session id in history");
        }
        if !std::path::Path::new(cwd).is_dir() {
            return ctx.fail(format!("Project folder no longer exists: {cwd}"));
        }
        let mut args = vec!["--resume", id];
        if remote {
            args.push("--remote-control");
        }
        os::run_in_terminal(cwd, &args)?;
        Ok(())
    }

    fn session_slice(s: &Session) -> Value {
        let state = match &s.detail {
            Some(d) => format!("{}: {d}", s.status.word()),
            None => s.status.word().to_string(),
        };
        json!({"id": s.id, "label": format!("{} · {state}", s.name), "icon": dots_icon(&[s.status])})
    }

    /// The sessions tile: one dot per live session; the label calls out anything that needs you.
    fn sessions_visual() -> Visual {
        let list = live_sessions();
        let count = |st: Status| list.iter().filter(|s| s.status == st).count();
        let attention = count(Status::Waiting) + count(Status::Blocked);
        let label = if attention > 0 { format!("{attention} need you") } else { format!("{}/{} running", count(Status::Running), list.len()) };
        Visual::new().icon(dots_icon(&list.iter().map(|s| s.status).collect::<Vec<_>>())).label(label)
    }

    /// Per-instance usage window index for the Usage button (press cycles it).
    static PERIOD: Mutex<Option<HashMap<String, usize>>> = Mutex::new(None);

    fn period_of(instance: &str) -> usize {
        PERIOD.lock().unwrap().as_ref().and_then(|m| m.get(instance).copied()).unwrap_or(0)
    }

    fn usage_visual(instance: &str) -> Visual {
        let i = period_of(instance);
        let (name, _) = usage::WINDOWS[i];
        match usage::snapshot() {
            Some(t) => Visual::new().label(format!("{name} · {} tok · {} msgs", usage::compact(t[i].tokens), t[i].messages)).icon("gauge"),
            None => Visual::new().label(format!("{name} · counting…")).icon("loader"),
        }
    }

    pub fn run() {
        Plugin::new("claude", "Claude")
            .version("1.0.0")
            .author("KnobDeck")
            .description("See and switch between running Claude Code sessions, turn on Remote Control, view usage and reopen recent sessions. Read-only: no prompts, no AI calls.")
            .icon("bot")
            .platforms(&["windows", "macos"])
            // ── Running sessions (dynamic wheel + live status dots on the tile) ──
            .action(
                Action::wheel("sessions", "Claude Sessions")
                    .icon("bot")
                    .live(LIVE, |_| Ok(sessions_visual()))
                    .on_open(|ctx| {
                        let list = sessions::sessions();
                        if list.is_empty() {
                            return ctx.fail("No Claude Code sessions running");
                        }
                        Ok(list.iter().map(session_slice).collect())
                    })
                    .on_child_press(|ctx| {
                        let Some(s) = ctx.child_id.as_deref().and_then(find_session) else { return ctx.fail("That session has ended") };
                        if !s.alive() {
                            return resume_in_terminal(ctx, &s.id, &s.cwd, false);
                        }
                        focus(ctx, &s)
                    }),
            )
            // ── Remote Control for the session in front (context aware) ──
            .action(
                Action::button("remote-control", "Remote Control")
                    .icon("radio-tower")
                    .live(LIVE, |_| {
                        let list = live_sessions();
                        Ok(match sessions::focused_or_latest(&list) {
                            Some(s) if s.bridge.is_some() => Visual::new().label(format!("{} · RC on", s.name)).icon("radio-tower"),
                            Some(s) => Visual::new().label(format!("{} · RC off", s.name)).icon("radio"),
                            None => Visual::new().label("No session").icon("circle-off"),
                        })
                    })
                    .on_press(|ctx| {
                        let list = live_sessions();
                        match sessions::focused_or_latest(&list) {
                            Some(s) => remote_control(ctx, s),
                            None => ctx.fail("No Claude Code sessions running"),
                        }
                    }),
            )
            .action(
                Action::wheel("remote-control-wheel", "Remote Control…")
                    .icon("radio-tower")
                    .on_open(|ctx| {
                        let list = live_sessions();
                        if list.is_empty() {
                            return ctx.fail("No Claude Code sessions running");
                        }
                        Ok(list
                            .iter()
                            .map(|s| {
                                let (state, icon) = if s.bridge.is_some() { ("RC on", "radio-tower") } else { ("RC off", "radio") };
                                json!({"id": s.id, "label": format!("{} · {state}", s.name), "icon": icon})
                            })
                            .collect())
                    })
                    .on_child_press(|ctx| match ctx.child_id.as_deref().and_then(find_session) {
                        Some(s) => remote_control(ctx, &s),
                        None => ctx.fail("That session has ended"),
                    }),
            )
            // ── Usage stats ──
            .action(
                Action::button("usage", "Usage")
                    .icon("gauge")
                    .live(LIVE, |ic| Ok(usage_visual(&ic.instance_id)))
                    .on_press(|ctx| {
                        let mut g = PERIOD.lock().unwrap();
                        let m = g.get_or_insert_with(HashMap::new);
                        let next = (m.get(&ctx.instance_id).copied().unwrap_or(0) + 1) % usage::WINDOWS.len();
                        m.insert(ctx.instance_id.clone(), next);
                        drop(g);
                        ctx.set_visual(usage_visual(&ctx.instance_id));
                        Ok(())
                    }),
            )
            // ── Recent sessions from history (last 5) ──
            .action(
                Action::wheel("recent", "Recent Sessions")
                    .icon("history")
                    .inputs(vec![input::select("open_in", "Open in", &[("terminal", "Claude Code (terminal)"), ("app", "Claude app (claude.ai/code)")]).default_value("terminal")])
                    .on_open(|ctx| {
                        let items = sessions::recent(5);
                        if items.is_empty() {
                            return ctx.fail("No Claude history yet");
                        }
                        let live = sessions::sessions();
                        Ok(items
                            .iter()
                            .map(|h| {
                                let label: String = h.label.chars().take(40).collect();
                                let icon = live.iter().find(|s| s.id == h.id).map_or_else(|| "message-square".to_string(), |s| dots_icon(&[s.status]));
                                json!({"id": h.id, "label": format!("{} · {label}", folder_name(&h.project)), "icon": icon})
                            })
                            .collect())
                    })
                    .on_child_press(|ctx| {
                        let Some(id) = ctx.child_id.clone() else { return Ok(()) };
                        let app = ctx.input_str("open_in") == "app";
                        // Already running: go to it instead of opening a second copy.
                        if let Some(s) = find_session(&id).filter(Session::alive) {
                            return if app { remote_control(ctx, &s) } else { focus(ctx, &s) };
                        }
                        let Some(h) = sessions::recent(50).into_iter().find(|h| h.id == id) else { return ctx.fail("That session is no longer in history") };
                        resume_in_terminal(ctx, &h.id, &h.project, app)?;
                        if app {
                            open_url("https://claude.ai/code")?;
                        }
                        Ok(())
                    }),
            )
            .on_ready(|rt| {
                // Claude Code rewrites sessions/<pid>.json on every status change: push it at once.
                let rt = rt.clone();
                std::thread::spawn(move || os::watch_dir(&sessions::claude_dir().join("sessions"), || rt.notify_visual("sessions", sessions_visual())));
                std::thread::spawn(|| loop {
                    let t = std::time::Instant::now();
                    usage::refresh();
                    log(format!("usage refreshed in {:?}", t.elapsed()));
                    std::thread::sleep(Duration::from_secs(60));
                });
                Ok(())
            })
            .run()
    }
}
