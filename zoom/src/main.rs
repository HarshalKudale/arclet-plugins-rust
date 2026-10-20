//! Zoom plugin for Arclet — controls Zoom via its local REST API
//! (Zoom Local App API, `localhost:19421`, available while a meeting is active).

use arclet_sdk::input;
use arclet_sdk::{json, log, Action, Ctx, Error, Plugin, Result, Runtime, Value};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Duration;

const ZOOM_BASE: &str = "http://localhost:19421/v2";
const POLL_INTERVAL: Duration = Duration::from_secs(3);

#[derive(Default, Clone, PartialEq)]
struct State {
    in_meeting: bool,
    muted: bool,
    video_off: bool,
    sharing: bool,
    hand_raised: bool,
    recording: bool,
    participants_open: bool,
    chat_open: bool,
}

static STATE: Mutex<State> = Mutex::new(State {
    in_meeting: false,
    muted: false,
    video_off: false,
    sharing: false,
    hand_raised: false,
    recording: false,
    participants_open: false,
    chat_open: false,
});
static RUNTIME: OnceLock<Runtime> = OnceLock::new();

fn st() -> MutexGuard<'static, State> {
    STATE.lock().unwrap()
}

fn pick<'a>(b: bool, yes: &'a str, no: &'a str) -> &'a str {
    if b {
        yes
    } else {
        no
    }
}

fn zoom_request(method: &str, path: &str, body: Option<Value>) -> Result<Value> {
    let req = ureq::request(method, &format!("{ZOOM_BASE}{path}")).timeout(Duration::from_secs(10));
    let res = match body {
        Some(b) => req.send_json(b),
        None => req.call(),
    };
    match res {
        Ok(r) => Ok(r.into_json().unwrap_or_else(|_| json!({}))),
        Err(ureq::Error::Status(code, r)) => {
            Err(Error::msg(format!("Zoom API error {code}: {}", r.into_string().unwrap_or_default())))
        }
        Err(e) => Err(Error::msg(format!(
            "Zoom is not reachable — make sure Zoom is open and you are in a meeting. ({e})"
        ))),
    }
}

fn patch(path: &str, body: Value) -> Result<Value> {
    zoom_request("PATCH", path, Some(body))
}

/// Apply a fetched status (or `None` when Zoom is unreachable) to local state and push updates.
fn apply_status(data: Option<Value>) {
    let mut s = st();
    let panels = (s.participants_open, s.chat_open);
    let next = match data {
        None if !s.in_meeting => return,
        None => State { participants_open: panels.0, chat_open: panels.1, ..State::default() },
        Some(d) => State {
            in_meeting: d["status"] == "inMeeting",
            muted: d["audio"]["isMuted"].as_bool().unwrap_or(false),
            video_off: !d["video"]["isStarted"].as_bool().unwrap_or(false),
            sharing: d["sharing"]["isSharingScreen"].as_bool().unwrap_or(false),
            hand_raised: d["handRaised"].as_bool().unwrap_or(false),
            recording: d["recording"]["status"] == "started",
            participants_open: panels.0,
            chat_open: panels.1,
        },
    };
    if *s != next {
        *s = next;
        drop(s);
        push_all_states();
    }
}

/// Fan out all current states to every placed action instance.
fn push_all_states() {
    let Some(rt) = RUNTIME.get() else { return };
    let s = st().clone();
    rt.notify("meeting-status", pick(s.in_meeting, "in-meeting", "idle"));
    rt.notify("toggle-mute", pick(s.muted, "muted", "unmuted"));
    rt.notify("toggle-video", pick(s.video_off, "off", "on"));
    rt.notify("toggle-screen-share", pick(s.sharing, "sharing", "idle"));
    rt.notify("raise-hand", pick(s.hand_raised, "raised", "lowered"));
    rt.notify("toggle-recording", pick(s.recording, "recording", "stopped"));
}

fn poll() {
    apply_status(zoom_request("GET", "/meeting_status", None).ok());
}

fn require_meeting(ctx: &Ctx) -> Result {
    if !st().in_meeting {
        let msg = "Not in a Zoom meeting.";
        ctx.set_error(msg);
        return Err(Error::msg(msg));
    }
    Ok(())
}

/// A toggle whose state mirrors one boolean in `State`, flipped via a Zoom PATCH.
fn meeting_toggle(
    id: &str,
    label: &str,
    icon: &str,
    states: [(&str, &str, &str); 2],
    get: fn(&State) -> bool,
    set: fn(&mut State, bool),
    request: fn(bool) -> Result<Value>,
) -> Action {
    let mut a = Action::toggle(id, label).icon(icon);
    for (v, l, i) in states {
        a = a.state(v, l, i);
    }
    let on_value = states[1].0.to_string();
    let off_value = states[0].0.to_string();
    let initial_on = on_value.clone();
    let initial_off = off_value.clone();
    a.initial_with(move |_| Some(if get(&st()) { initial_on.clone() } else { initial_off.clone() }))
        .on_press(move |ctx| {
            require_meeting(ctx)?;
            let now = get(&st());
            request(now)?;
            let next = !now;
            set(&mut st(), next);
            ctx.fan_out(if next { &on_value } else { &off_value });
            Ok(())
        })
}

fn reset_meeting() {
    let mut s = st();
    *s = State { participants_open: s.participants_open, chat_open: s.chat_open, ..State::default() };
}

fn main() {
    Plugin::new("zoom", "Zoom")
        .version("1.0.0")
        .author("Arclet")
        .description("Control Zoom — mute, video, screen share, raise hand, recording & more.")
        .icon("icon:thesvg/zoom")
        .ack_timeout_ms(5000)
        .on_ready(|rt| {
            let _ = RUNTIME.set(rt.clone());
            log("Zoom plugin ready — starting status poller");
            std::thread::spawn(|| loop {
                poll();
                std::thread::sleep(POLL_INTERVAL);
            });
            Ok(())
        })
        // 1. Meeting status indicator
        .action(
            Action::toggle("meeting-status", "Meeting Status")
                .icon("meeting_status.svg")
                .state("idle", "Not in Meeting", "meeting_status.svg")
                .state("in-meeting", "In Meeting", "meeting_status.svg")
                .initial_with(|_| Some(pick(st().in_meeting, "in-meeting", "idle").into()))
                .on_press(|ctx| {
                    poll(); // re-poll on press for fresh state
                    ctx.fan_out(pick(st().in_meeting, "in-meeting", "idle"));
                    Ok(())
                }),
        )
        // 2. Mute
        .action(meeting_toggle(
            "toggle-mute",
            "Toggle Mute",
            "mute_off.svg",
            [("unmuted", "Unmuted", "mute_off.svg"), ("muted", "Muted", "mute_on.svg")],
            |s| s.muted,
            |s, v| s.muted = v,
            |muted| patch("/audio", json!({"action": pick(muted, "unmute", "mute")})),
        ))
        // 3. Push-to-talk
        .action(
            Action::hold("push-to-talk", "Push to Talk")
                .icon("mute_off.svg")
                .released("Idle", "mute_off.svg")
                .held("Talking", "mute_off.svg")
                .on_hold_start(|ctx| {
                    if !st().in_meeting {
                        return Ok(());
                    }
                    if st().muted {
                        let _ = patch("/audio", json!({"action": "unmute"}));
                    }
                    ctx.set_state("held");
                    Ok(())
                })
                .on_hold_end(|ctx| {
                    if !st().in_meeting {
                        return Ok(());
                    }
                    let _ = patch("/audio", json!({"action": "mute"}));
                    st().muted = true;
                    ctx.runtime().notify("toggle-mute", "muted");
                    ctx.set_state("released");
                    Ok(())
                }),
        )
        // 4. Video
        .action(meeting_toggle(
            "toggle-video",
            "Toggle Video",
            "video_on.svg",
            [("on", "Camera On", "video_on.svg"), ("off", "Camera Off", "video_off.svg")],
            |s| s.video_off,
            |s, v| s.video_off = v,
            |off| patch("/video", json!({"action": pick(off, "startVideo", "stopVideo")})),
        ))
        // 5. Screen share
        .action(meeting_toggle(
            "toggle-screen-share",
            "Screen Share",
            "share_off.svg",
            [("idle", "Not Sharing", "share_off.svg"), ("sharing", "Sharing", "share_on.svg")],
            |s| s.sharing,
            |s, v| s.sharing = v,
            |sharing| patch("/sharing", json!({"action": pick(sharing, "stopShare", "startShare")})),
        ))
        // 6. Hand
        .action(meeting_toggle(
            "raise-hand",
            "Raise / Lower Hand",
            "hand_raise.svg",
            [("lowered", "Hand Down", "hand_raise.svg"), ("raised", "Hand Up", "hand_raise.svg")],
            |s| s.hand_raised,
            |s, v| s.hand_raised = v,
            |raised| patch("/reaction", json!({"action": pick(raised, "lowerHand", "raiseHand")})),
        ))
        // 7. Emoji reaction
        .action(
            Action::button("react-emoji", "React (Emoji)")
                .icon("hand_raise.svg")
                .inputs(vec![input::select(
                    "emoji",
                    "Emoji",
                    &[
                        ("yes", "👍 Yes / Thumbs Up"),
                        ("no", "👎 No / Thumbs Down"),
                        ("clap", "👏 Clap"),
                        ("heart", "❤️ Heart"),
                        ("surprised", "😮 Surprised"),
                        ("laugh", "😂 Laugh / Joy"),
                        ("party", "🎉 Celebrate / Party Popper"),
                    ],
                )])
                .on_press(|ctx| {
                    require_meeting(ctx)?;
                    let emoji = match ctx.input_str("emoji") {
                        e if e.is_empty() => "yes".to_string(),
                        e => e,
                    };
                    patch("/reaction", json!({"action": "sendEmoji", "emoji": emoji}))?;
                    Ok(())
                }),
        )
        // 8. Recording
        .action(meeting_toggle(
            "toggle-recording",
            "Record",
            "record.svg",
            [("stopped", "Not Recording", "record.svg"), ("recording", "Recording", "record_stop.svg")],
            |s| s.recording,
            |s, v| s.recording = v,
            |rec| patch("/recording", json!({"action": pick(rec, "stop", "start")})),
        ))
        // 9–10. Leave / End
        .action(meeting_exit("leave-meeting", "Leave Meeting", "leave.svg", "leave"))
        .action(meeting_exit("end-meeting", "End Meeting (Host)", "end_meeting.svg", "end"))
        // 11–12. Panels
        .action(meeting_toggle(
            "toggle-participants",
            "Participants Panel",
            "participants.svg",
            [("closed", "Closed", "participants.svg"), ("open", "Open", "participants.svg")],
            |s| s.participants_open,
            |s, v| s.participants_open = v,
            |open| patch("/participants", json!({"action": pick(open, "closeParticipantPanel", "openParticipantPanel")})),
        ))
        .action(meeting_toggle(
            "toggle-chat",
            "Chat Panel",
            "chat.svg",
            [("closed", "Closed", "chat.svg"), ("open", "Open", "chat.svg")],
            |s| s.chat_open,
            |s, v| s.chat_open = v,
            |open| patch("/chat", json!({"action": pick(open, "closeChat", "openChat")})),
        ))
        .run()
}

fn meeting_exit(id: &str, label: &str, icon: &str, action: &'static str) -> Action {
    Action::button(id, label).icon(icon).on_press(move |ctx| {
        require_meeting(ctx)?;
        patch("/meeting", json!({"action": action}))?;
        reset_meeting();
        push_all_states();
        Ok(())
    })
}
