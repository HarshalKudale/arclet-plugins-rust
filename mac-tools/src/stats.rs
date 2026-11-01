//! Live CPU / GPU / memory use for the System Stats tile.
//!
//! - CPU: `host_statistics(HOST_CPU_LOAD_INFO)` tick deltas between samples.
//! - Memory: "Memory Used" as Activity Monitor computes it (app + wired + compressed).
//! - GPU: IOKit `IOAccelerator` → `PerformanceStatistics["Device Utilization %"]`
//!   (Apple silicon, AMD and Intel drivers all publish it); the busiest GPU wins.

use arclet_sdk::Result;
use base64::Engine;
use core_foundation::base::{CFType, TCFType};
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
use core_foundation::number::CFNumber;
use core_foundation::string::{CFString, CFStringRef};
use std::ffi::c_void;
use std::sync::Mutex;

// ── CPU ──

static LAST_TICKS: Mutex<Option<[u64; 4]>> = Mutex::new(None);

fn cpu_ticks() -> Option<[u64; 4]> {
    let mut info: libc::host_cpu_load_info = unsafe { std::mem::zeroed() };
    let mut count = (std::mem::size_of::<libc::host_cpu_load_info>() / std::mem::size_of::<libc::integer_t>()) as libc::mach_msg_type_number_t;
    #[allow(deprecated)]
    let host = unsafe { libc::mach_host_self() };
    let kr = unsafe { libc::host_statistics(host, libc::HOST_CPU_LOAD_INFO, &mut info as *mut _ as libc::host_info_t, &mut count) };
    (kr == 0).then(|| info.cpu_ticks.map(|t| t as u64))
}

/// Percent busy since the previous call; `None` on the first (warm-up) call.
fn cpu() -> Option<f64> {
    let now = cpu_ticks()?;
    let prev = LAST_TICKS.lock().unwrap().replace(now)?;
    let d: Vec<u64> = now.iter().zip(prev).map(|(n, p)| n.wrapping_sub(p)).collect();
    let total: u64 = d.iter().sum();
    let idle = d[libc::CPU_STATE_IDLE as usize];
    (total > 0).then(|| 100.0 * (total - idle) as f64 / total as f64)
}

// ── Memory ──

fn total_memory() -> Option<u64> {
    let mut v: u64 = 0;
    let mut len = std::mem::size_of::<u64>();
    let name = c"hw.memsize";
    let r = unsafe { libc::sysctlbyname(name.as_ptr(), &mut v as *mut _ as *mut c_void, &mut len, std::ptr::null_mut(), 0) };
    (r == 0 && v > 0).then_some(v)
}

fn memory() -> Option<f64> {
    let mut vm: libc::vm_statistics64 = unsafe { std::mem::zeroed() };
    let mut count = libc::HOST_VM_INFO64_COUNT;
    #[allow(deprecated)]
    let host = unsafe { libc::mach_host_self() };
    let kr = unsafe { libc::host_statistics64(host, libc::HOST_VM_INFO64, &mut vm as *mut _ as libc::host_info64_t, &mut count) };
    if kr != 0 {
        return None;
    }
    #[allow(deprecated)]
    let page = unsafe { libc::vm_page_size } as u64;
    let app = (vm.internal_page_count as u64).saturating_sub(vm.purgeable_count as u64);
    let used = (app + vm.wire_count as u64 + vm.compressor_page_count as u64) * page;
    let total = total_memory()?;
    Some((100.0 * used as f64 / total as f64).clamp(0.0, 100.0))
}

// ── GPU ──

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOServiceMatching(name: *const libc::c_char) -> CFDictionaryRef;
    fn IOServiceGetMatchingServices(main_port: u32, matching: CFDictionaryRef, iter: *mut u32) -> i32;
    fn IOIteratorNext(iter: u32) -> u32;
    fn IOObjectRelease(obj: u32) -> i32;
    fn IORegistryEntryCreateCFProperty(entry: u32, key: CFStringRef, alloc: *const c_void, options: u32) -> *const c_void;
}

fn gpu() -> Option<f64> {
    let mut iter = 0u32;
    // IOServiceGetMatchingServices consumes the matching dictionary.
    let matching = unsafe { IOServiceMatching(c"IOAccelerator".as_ptr()) };
    if matching.is_null() || unsafe { IOServiceGetMatchingServices(0, matching, &mut iter) } != 0 {
        return None;
    }
    let key = CFString::from_static_string("PerformanceStatistics");
    let util_key = CFString::from_static_string("Device Utilization %");
    let mut best: Option<f64> = None;
    loop {
        let entry = unsafe { IOIteratorNext(iter) };
        if entry == 0 {
            break;
        }
        let raw = unsafe { IORegistryEntryCreateCFProperty(entry, key.as_concrete_TypeRef(), std::ptr::null(), 0) };
        unsafe { IOObjectRelease(entry) };
        if raw.is_null() {
            continue;
        }
        let v = unsafe { CFType::wrap_under_create_rule(raw) };
        if let Some(dict) = v.downcast::<CFDictionary>() {
            let dict: CFDictionary<CFString, CFType> = unsafe { CFDictionary::wrap_under_get_rule(dict.as_concrete_TypeRef()) };
            if let Some(p) = dict.find(&util_key).and_then(|n| n.downcast::<CFNumber>()).and_then(|n| n.to_f64()) {
                best = Some(best.map_or(p, |b: f64| b.max(p)));
            }
        }
    }
    unsafe { IOObjectRelease(iter) };
    best.map(|p| p.clamp(0.0, 100.0))
}

/// `(short name, percent)` for `stat` (`cpu` | `gpu` | `memory`); unknown → cpu.
/// `None` percent while the first CPU sample is still warming up.
pub fn read(stat: &str) -> Result<(&'static str, Option<f64>)> {
    Ok(match stat {
        "memory" => ("RAM", memory()),
        "gpu" => ("GPU", gpu()),
        _ => ("CPU", cpu()),
    })
}

/// Ring gauge with the number in the middle, as an SVG data URI (same look as Win Tools).
pub fn gauge(pct: f64) -> String {
    let p = pct.clamp(0.0, 100.0).round();
    let color = if p >= 85.0 { "#ef4444" } else if p >= 60.0 { "#f59e0b" } else { "#22c55e" };
    let circ = 2.0 * std::f64::consts::PI * 40.0;
    let svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100"><circle cx="50" cy="50" r="40" fill="none" stroke="#888888" stroke-opacity="0.3" stroke-width="10"/><circle cx="50" cy="50" r="40" fill="none" stroke="{color}" stroke-width="10" stroke-linecap="round" stroke-dasharray="{:.1} {circ:.1}" transform="rotate(-90 50 50)"/><text x="50" y="61" text-anchor="middle" font-family="-apple-system, system-ui, sans-serif" font-size="30" font-weight="600" fill="#888888">{p:.0}</text></svg>"##,
        circ * p / 100.0
    );
    format!("data:image/svg+xml;base64,{}", base64::engine::general_purpose::STANDARD.encode(svg))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_reads_after_warmup() {
        let _ = read("cpu").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        let (name, cpu) = read("cpu").unwrap();
        assert_eq!(name, "CPU");
        assert!((0.0..=100.0).contains(&cpu.unwrap()));
        eprintln!("cpu={cpu:?} gpu={:?} mem={:?}", read("gpu").unwrap().1, read("memory").unwrap().1);
    }

    #[test]
    fn memory_is_a_percent() {
        let m = memory().unwrap();
        assert!(m > 0.0 && m <= 100.0);
    }

    #[test]
    fn gauge_encodes() {
        assert!(gauge(42.0).starts_with("data:image/svg+xml;base64,"));
    }
}
