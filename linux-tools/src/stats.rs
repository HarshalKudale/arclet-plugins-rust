//! CPU, GPU and memory use from the kernel (`/proc`, DRM sysfs), plus NVIDIA's own tool.

use base64::Engine;
use knobdeck_sdk::{Error, Result};
use std::sync::Mutex;

/// The last CPU sample: (busy, total) jiffies.
static LAST_CPU: Mutex<Option<(u64, u64)>> = Mutex::new(None);

fn cpu_sample() -> Option<(u64, u64)> {
    let stat = std::fs::read_to_string("/proc/stat").ok()?;
    let nums: Vec<u64> = stat.lines().next()?.split_whitespace().skip(1).filter_map(|n| n.parse().ok()).collect();
    // user nice system idle iowait irq softirq steal (guest time is already in user/nice).
    let total: u64 = nums.iter().take(8).sum();
    let idle = nums.get(3)? + nums.get(4).unwrap_or(&0);
    Some((total - idle, total))
}

/// CPU use since the previous call (the first call only primes the sample).
fn cpu() -> Option<f64> {
    let now = cpu_sample()?;
    let prev = LAST_CPU.lock().unwrap().replace(now)?;
    let (busy, total) = (now.0.saturating_sub(prev.0), now.1.saturating_sub(prev.1));
    (total > 0).then(|| busy as f64 * 100.0 / total as f64)
}

fn memory() -> Option<f64> {
    let info = std::fs::read_to_string("/proc/meminfo").ok()?;
    let field = |k: &str| info.lines().find_map(|l| l.strip_prefix(k)?.split_whitespace().next()?.parse::<f64>().ok());
    let (total, avail) = (field("MemTotal:")?, field("MemAvailable:")?);
    (total > 0.0).then(|| (total - avail) * 100.0 / total)
}

/// AMD and Intel (xe) report `gpu_busy_percent` in sysfs; NVIDIA through nvidia-smi.
fn gpu() -> Option<f64> {
    let busiest = std::fs::read_dir("/sys/class/drm")
        .ok()?
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("card") && !e.file_name().to_string_lossy().contains('-'))
        .filter_map(|e| std::fs::read_to_string(e.path().join("device/gpu_busy_percent")).ok()?.trim().parse::<f64>().ok())
        .reduce(f64::max);
    if busiest.is_some() {
        return busiest;
    }
    let out = std::process::Command::new("nvidia-smi").args(["--query-gpu=utilization.gpu", "--format=csv,noheader,nounits"]).output().ok()?;
    String::from_utf8_lossy(&out.stdout).lines().filter_map(|l| l.trim().parse::<f64>().ok()).reduce(f64::max)
}

/// (display name, percent) for `cpu`, `gpu` or `memory`; `None` while unavailable.
pub fn read(stat: &str) -> Result<(&'static str, Option<f64>)> {
    Ok(match stat {
        "gpu" => ("GPU", gpu()),
        "memory" => ("Memory", memory()),
        "cpu" | "" => ("CPU", cpu()),
        other => return Err(Error::msg(format!("Unknown stat \"{other}\""))),
    })
}

/// A ring gauge as an SVG data URI (same look as Win Tools / Mac Tools).
pub fn gauge(pct: f64) -> String {
    let p = pct.clamp(0.0, 100.0).round();
    let color = if p >= 85.0 { "#ef4444" } else if p >= 60.0 { "#f59e0b" } else { "#22c55e" };
    let circ = 2.0 * std::f64::consts::PI * 40.0;
    let svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100"><circle cx="50" cy="50" r="40" fill="none" stroke="#888888" stroke-opacity="0.3" stroke-width="10"/><circle cx="50" cy="50" r="40" fill="none" stroke="{color}" stroke-width="10" stroke-linecap="round" stroke-dasharray="{:.1} {circ:.1}" transform="rotate(-90 50 50)"/><text x="50" y="61" text-anchor="middle" font-family="sans-serif" font-size="30" font-weight="600" fill="#888888">{p:.0}</text></svg>"##,
        circ * p / 100.0
    );
    format!("data:image/svg+xml;base64,{}", base64::engine::general_purpose::STANDARD.encode(svg))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_stats() {
        let _ = read("cpu").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        let cpu = read("cpu").unwrap().1.unwrap();
        assert!((0.0..=100.0).contains(&cpu));
        let mem = read("memory").unwrap().1.unwrap();
        assert!((0.0..=100.0).contains(&mem));
        eprintln!("cpu={cpu:.1} mem={mem:.1} gpu={:?}", read("gpu").unwrap().1);
        assert!(read("disk").is_err());
        assert!(gauge(150.0).starts_with("data:image/svg+xml;base64,"));
    }
}
