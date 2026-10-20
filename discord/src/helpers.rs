//! Pure helpers for labelling and picking Discord objects.

use arclet_sdk::Value;

fn s<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v[k].as_str().filter(|x| !x.is_empty())
}

pub fn display_name_for_user(user: &Value) -> String {
    ["global_name", "globalName", "display_name", "displayName", "username", "id"]
        .iter()
        .find_map(|k| s(user, k))
        .unwrap_or("Unknown User")
        .to_string()
}

pub fn format_relationship_label(rel: &Value) -> String {
    let user = &rel["user"];
    let primary = display_name_for_user(user);
    match s(user, "username") {
        Some(u) if u != primary => format!("{primary} (@{u})"),
        _ => primary,
    }
}

/// The DM (type 1) or group DM (type 3) channel that includes the relationship's user.
pub fn find_direct_message_channel<'a>(channels: &'a [Value], rel: &Value) -> Option<&'a Value> {
    let uid = s(&rel["user"], "id")?;
    channels.iter().find(|c| {
        matches!(c["type"].as_i64(), Some(1 | 3)) && c["recipients"].as_array().map_or(false, |r| r.iter().any(|x| x["id"] == uid))
    })
}

pub fn find_best_text_channel(channels: &[Value]) -> Option<&Value> {
    channels
        .iter()
        .filter(|c| c["type"] == 0)
        .min_by_key(|c| (c["position"].as_i64().unwrap_or(i64::MAX), c["name"].as_str().unwrap_or("").to_lowercase()))
}

pub fn format_device_label(d: &Value) -> String {
    let name = ["name", "label", "id"].iter().find_map(|k| s(d, k)).unwrap_or("Unknown Device");
    if d["id"] == "default" {
        format!("{name} (Default)")
    } else {
        name.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arclet_sdk::json;

    #[test]
    fn labels_and_picking() {
        let rel = json!({"user": {"id": "1", "global_name": "Ann", "username": "ann99"}});
        assert_eq!(format_relationship_label(&rel), "Ann (@ann99)");
        let chans = [
            json!({"id": "b", "type": 0, "name": "zeta", "position": 2}),
            json!({"id": "a", "type": 0, "name": "alpha", "position": 1}),
            json!({"id": "v", "type": 2}),
        ];
        assert_eq!(find_best_text_channel(&chans).unwrap()["id"], "a");
        let dms = [json!({"id": "d", "type": 1, "recipients": [{"id": "1"}]})];
        assert_eq!(find_direct_message_channel(&dms, &rel).unwrap()["id"], "d");
        assert_eq!(format_device_label(&json!({"id": "default", "name": "Mic"})), "Mic (Default)");
    }
}
