//! Live system stats (CPU / GPU / memory %) for the "System Stats" button, plus
//! a ring-gauge icon to show them on the tile.

use arclet_sdk::{Error, Result};
use base64::Engine;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use windows::core::w;
use windows::Win32::System::Performance::{
    PdhAddEnglishCounterW, PdhCollectQueryData, PdhGetFormattedCounterArrayW, PdhGetFormattedCounterValue, PdhOpenQueryW, PDH_FMT_COUNTERVALUE, PDH_FMT_COUNTERVALUE_ITEM_W, PDH_FMT_DOUBLE,
    PDH_MORE_DATA,
};
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};

/// One PDH query shared by every tile; re-collected at most every 500 ms so
/// several stats tiles on one tick read the same sample.
struct Sampler {
    query: isize,
    cpu: Option<isize>,
    gpu: Option<isize>,
    at: Option<Instant>,
    last: (Option<f64>, Option<f64>),
}

static SAMPLER: Mutex<Option<Sampler>> = Mutex::new(None);

impl Sampler {
    fn open() -> Result<Self> {
        let mut query = 0isize;
        if unsafe { PdhOpenQueryW(None, 0, &mut query) } != 0 {
            return Err(Error::msg("Could not open performance counters"));
        }
        let add = |path| {
            let mut c = 0isize;
            (unsafe { PdhAddEnglishCounterW(query, path, 0, &mut c) } == 0).then_some(c)
        };
        // "% Processor Utility" is what Task Manager shows; older systems only have "% Processor Time".
        let cpu = add(w!("\\Processor Information(_Total)\\% Processor Utility")).or_else(|| add(w!("\\Processor(_Total)\\% Processor Time")));
        let gpu = add(w!("\\GPU Engine(*engtype_3D)\\Utilization Percentage"));
        Ok(Sampler { query, cpu, gpu, at: None, last: (None, None) })
    }

    fn sample(&mut self) -> (Option<f64>, Option<f64>) {
        if self.at.is_some_and(|t| t.elapsed() < Duration::from_millis(500)) {
            return self.last;
        }
        self.at = Some(Instant::now());
        // Rate counters need two collections; the first tick reads nothing.
        if unsafe { PdhCollectQueryData(self.query) } == 0 {
            self.last = (self.cpu.and_then(single), self.gpu.and_then(summed));
        }
        self.last
    }
}

fn single(counter: isize) -> Option<f64> {
    let mut v = PDH_FMT_COUNTERVALUE::default();
    (unsafe { PdhGetFormattedCounterValue(counter, PDH_FMT_DOUBLE, None, &mut v) } == 0 && v.CStatus == 0).then(|| unsafe { v.Anonymous.doubleValue })
}

// shortcut: sums 3D-engine utilization across all GPU processes (Task Manager takes the busiest engine type per adapter); enough for a tile, refine if users compare numbers.
fn summed(counter: isize) -> Option<f64> {
    let (mut size, mut count) = (0u32, 0u32);
    if unsafe { PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE, &mut size, &mut count, None) } != PDH_MORE_DATA {
        return None;
    }
    // u64 storage keeps the item array 8-byte aligned.
    let mut buf = vec![0u64; (size as usize).div_ceil(8)];
    let items = buf.as_mut_ptr() as *mut PDH_FMT_COUNTERVALUE_ITEM_W;
    if unsafe { PdhGetFormattedCounterArrayW(counter, PDH_FMT_DOUBLE, &mut size, &mut count, Some(items)) } != 0 {
        return None;
    }
    let items = unsafe { std::slice::from_raw_parts(items, count as usize) };
    Some(items.iter().filter(|i| i.FmtValue.CStatus == 0).map(|i| unsafe { i.FmtValue.Anonymous.doubleValue }).sum())
}

fn memory() -> Option<f64> {
    let mut m = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    unsafe { GlobalMemoryStatusEx(&mut m) }.ok()?;
    (m.ullTotalPhys > 0).then(|| 100.0 * (m.ullTotalPhys - m.ullAvailPhys) as f64 / m.ullTotalPhys as f64)
}

/// `(short name, percent)` for `stat` (`cpu` | `gpu` | `memory`); unknown → cpu.
/// `None` percent while the first sample is still warming up.
pub fn read(stat: &str) -> Result<(&'static str, Option<f64>)> {
    if stat == "memory" {
        return Ok(("RAM", memory()));
    }
    let mut guard = SAMPLER.lock().unwrap();
    if guard.is_none() {
        *guard = Some(Sampler::open()?);
    }
    let (cpu, gpu) = guard.as_mut().unwrap().sample();
    Ok(if stat == "gpu" { ("GPU", gpu) } else { ("CPU", cpu) })
}

/// Ring gauge with the number in the middle, as an SVG data URI (works on the
/// phone deck and the dial overlay; a fixed mid-grey reads on light and dark).
pub fn gauge(pct: f64) -> String {
    let p = pct.clamp(0.0, 100.0).round();
    let color = if p >= 85.0 { "#ef4444" } else if p >= 60.0 { "#f59e0b" } else { "#22c55e" };
    let circ = 2.0 * std::f64::consts::PI * 40.0;
    let svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100"><circle cx="50" cy="50" r="40" fill="none" stroke="#888888" stroke-opacity="0.3" stroke-width="10"/><circle cx="50" cy="50" r="40" fill="none" stroke="{color}" stroke-width="10" stroke-linecap="round" stroke-dasharray="{:.1} {circ:.1}" transform="rotate(-90 50 50)"/><text x="50" y="61" text-anchor="middle" font-family="Segoe UI, sans-serif" font-size="30" font-weight="600" fill="#888888">{p:.0}</text></svg>"##,
        circ * p / 100.0
    );
    format!("data:image/svg+xml;base64,{}", base64::engine::general_purpose::STANDARD.encode(svg))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gauge_clamps_and_encodes() {
        let decode = |u: String| String::from_utf8(base64::engine::general_purpose::STANDARD.decode(u.trim_start_matches("data:image/svg+xml;base64,")).unwrap()).unwrap();
        assert!(decode(gauge(150.0)).contains(">100<"));
        assert!(decode(gauge(-3.0)).contains(">0<"));
        assert!(decode(gauge(42.4)).contains("#22c55e"));
    }

    #[test]
    fn cpu_reads_after_warmup() {
        let _ = read("cpu").unwrap();
        std::thread::sleep(Duration::from_millis(600));
        let (name, cpu) = read("cpu").unwrap();
        assert_eq!(name, "CPU");
        assert!((0.0..=100.0).contains(&cpu.unwrap()));
        eprintln!("cpu={cpu:?} gpu={:?}", read("gpu").unwrap().1);
    }

    #[test]
    fn memory_is_a_percent() {
        let m = memory().unwrap();
        assert!((0.0..=100.0).contains(&m));
    }
}
