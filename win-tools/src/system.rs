//! Wi-Fi / Bluetooth radios (WinRT `Windows.Devices.Radios`), power plans (powrprof) and ping (ICMP).

use knobdeck_sdk::{Error, Result};
use windows::core::GUID;
use windows::Devices::Radios::{Radio, RadioKind, RadioState};
use windows::Win32::Foundation::{LocalFree, ERROR_SUCCESS, HLOCAL, WIN32_ERROR};
use windows::Win32::NetworkManagement::IpHelper::{IcmpCloseHandle, IcmpCreateFile, IcmpSendEcho, ICMP_ECHO_REPLY};
use windows::Win32::System::Power::{PowerEnumerate, PowerGetActiveScheme, PowerReadFriendlyName, PowerSetActiveScheme, ACCESS_SCHEME};

// ── Radios ──

fn radio_kind(kind: &str) -> RadioKind {
    if kind == "bluetooth" {
        RadioKind::Bluetooth
    } else {
        RadioKind::WiFi
    }
}

fn find_radio(kind: &str) -> Result<Radio> {
    crate::audio::ensure_com();
    let werr = |e: windows::core::Error| Error::msg(format!("Could not read the radios: {e}"));
    let radios = Radio::GetRadiosAsync().and_then(|op| op.get()).map_err(werr)?;
    let want = radio_kind(kind);
    radios.into_iter().find(|r| r.Kind().ok() == Some(want)).ok_or_else(|| Error::msg(if kind == "bluetooth" { "This PC has no Bluetooth radio" } else { "This PC has no Wi-Fi radio" }))
}

pub fn radio_on(kind: &str) -> Result<bool> {
    Ok(find_radio(kind)?.State().map_err(|e| Error::msg(format!("Radio state: {e}")))? == RadioState::On)
}

/// Turn the radio on or off; returns the new state.
pub fn set_radio(kind: &str, on: bool) -> Result<bool> {
    let radio = find_radio(kind)?;
    let status = radio
        .SetStateAsync(if on { RadioState::On } else { RadioState::Off })
        .and_then(|op| op.get())
        .map_err(|e| Error::msg(format!("Could not switch the radio: {e}")))?;
    if status != windows::Devices::Radios::RadioAccessStatus::Allowed {
        // DeniedBySystem: airplane mode or a hardware switch holds the radio off.
        return Err(Error::msg("Windows did not allow the change (airplane mode or a hardware switch may be on)"));
    }
    Ok(on)
}

// ── Power plans ──

fn win32(what: &str, e: WIN32_ERROR) -> Result<()> {
    if e == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(Error::msg(format!("{what} failed: {}", windows::core::Error::from(e.to_hresult()).message())))
    }
}

/// Power scheme GUIDs, in Windows' order, with their names.
pub fn power_plans() -> Result<Vec<(String, String)>> {
    let mut plans = vec![];
    for index in 0.. {
        let mut guid = GUID::zeroed();
        let mut size = std::mem::size_of::<GUID>() as u32;
        let r = unsafe { PowerEnumerate(None, None, None, ACCESS_SCHEME, index, Some(&mut guid as *mut GUID as *mut u8), &mut size) };
        if r != ERROR_SUCCESS {
            break; // ERROR_NO_MORE_ITEMS
        }
        plans.push((format!("{guid:?}"), plan_name(&guid).unwrap_or_else(|| format!("{guid:?}"))));
    }
    Ok(plans)
}

fn plan_name(guid: &GUID) -> Option<String> {
    let mut size = 0u32;
    // First call only reports the size needed.
    let _ = unsafe { PowerReadFriendlyName(None, Some(guid), None, None, None, &mut size) };
    let mut buf = vec![0u16; (size as usize).div_ceil(2)];
    let r = unsafe { PowerReadFriendlyName(None, Some(guid), None, None, Some(buf.as_mut_ptr() as *mut u8), &mut size) };
    (r == ERROR_SUCCESS).then(|| String::from_utf16_lossy(&buf).trim_end_matches('\0').to_string())
}

pub fn active_plan() -> Result<String> {
    let mut p: *mut GUID = std::ptr::null_mut();
    win32("PowerGetActiveScheme", unsafe { PowerGetActiveScheme(None, &mut p) })?;
    let guid = unsafe { *p };
    unsafe { LocalFree(HLOCAL(p as _)) };
    Ok(format!("{guid:?}"))
}

pub fn set_plan(id: &str) -> Result<()> {
    // GUID::try_from panics on malformed text, so parse the 32 hex digits ourselves.
    let hex: String = id.chars().filter(|c| *c != '-').collect();
    let guid = (hex.len() == 32).then(|| u128::from_str_radix(&hex, 16).ok()).flatten().map(GUID::from_u128).ok_or_else(|| Error::msg("Choose a power plan first"))?;
    win32("PowerSetActiveScheme", unsafe { PowerSetActiveScheme(None, Some(&guid)) })
}

// ── Ping ──

/// Round-trip time in ms to `host` (IPv4), or `None` on timeout.
pub fn ping(host: &str, timeout_ms: u32) -> Result<Option<u32>> {
    use std::net::{SocketAddr, ToSocketAddrs};
    let host = host.trim();
    if host.is_empty() {
        return Err(Error::msg("Enter a host to ping"));
    }
    let ip = (host, 0)
        .to_socket_addrs()
        .map_err(|_| Error::msg(format!("Unknown host \"{host}\"")))?
        .find_map(|a| match a {
            SocketAddr::V4(v4) => Some(*v4.ip()),
            _ => None,
        })
        .ok_or_else(|| Error::msg(format!("\"{host}\" has no IPv4 address")))?;
    let handle = unsafe { IcmpCreateFile() }.map_err(|e| Error::msg(format!("IcmpCreateFile failed: {e}")))?;
    let payload = [0u8; 32];
    // The reply buffer must hold one reply, the echoed data and 8 bytes for an ICMP error.
    let mut reply = vec![0u8; std::mem::size_of::<ICMP_ECHO_REPLY>() + payload.len() + 8];
    // IcmpSendEcho wants the address in network byte order as it sits in memory.
    let addr = u32::from_ne_bytes(ip.octets());
    let n = unsafe { IcmpSendEcho(handle, addr, payload.as_ptr() as _, payload.len() as u16, None, reply.as_mut_ptr() as _, reply.len() as u32, timeout_ms) };
    let result = (n > 0).then(|| {
        let r = unsafe { &*(reply.as_ptr() as *const ICMP_ECHO_REPLY) };
        (r.Status == 0).then_some(r.RoundTripTime)
    });
    let _ = unsafe { IcmpCloseHandle(handle) };
    Ok(result.flatten())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn power_plans_list_and_active_is_listed() {
        let plans = power_plans().unwrap();
        eprintln!("plans: {plans:?}");
        assert!(!plans.is_empty());
        let active = active_plan().unwrap();
        assert!(plans.iter().any(|(id, _)| *id == active), "active plan {active} is listed");
        assert!(set_plan("not-a-guid").is_err());
        // Re-selecting the active plan proves the id round-trips without changing anything.
        set_plan(&active).unwrap();
        assert_eq!(active_plan().unwrap(), active);
    }

    #[test]
    fn ping_loopback_and_bad_host() {
        assert!(ping("127.0.0.1", 1000).unwrap().is_some());
        assert!(ping("", 1000).is_err());
        assert!(ping("no-such-host.invalid", 1000).is_err());
    }

    /// Reads only; never switches a radio.
    #[test]
    fn radios_read() {
        for kind in ["wifi", "bluetooth"] {
            eprintln!("{kind}: {:?}", radio_on(kind).map_err(|e| e.message().to_string()));
        }
    }
}
