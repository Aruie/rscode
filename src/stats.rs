//! CPU and memory of this process, sampled every couple of seconds for the title bar.

use std::time::Instant;

pub const SAMPLES: usize = 60;

pub struct Stats {
    last_cpu: Option<(f64, Instant)>,
    cores: f64,
    pub cpu: Vec<f32>,
    pub mem_mb: Vec<f32>,
}

impl Stats {
    pub fn new() -> Self {
        let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1) as f64;
        Stats { last_cpu: None, cores, cpu: Vec::new(), mem_mb: Vec::new() }
    }

    /// Take one sample. CPU is a percentage of the whole machine, like Task Manager.
    pub fn sample(&mut self) {
        let now = Instant::now();
        let (cpu_secs, mem) = read_process();
        if let Some(c) = cpu_secs {
            if let Some((prev, at)) = self.last_cpu {
                let wall = now.duration_since(at).as_secs_f64();
                if wall > 0.0 {
                    let pct = ((c - prev) / wall / self.cores * 100.0).clamp(0.0, 100.0);
                    push(&mut self.cpu, pct as f32);
                }
            }
            self.last_cpu = Some((c, now));
        }
        if let Some(m) = mem {
            push(&mut self.mem_mb, (m as f64 / 1024.0 / 1024.0) as f32);
        }
    }

    pub fn label(&self) -> String {
        let cpu = self.cpu.last().copied().unwrap_or(0.0);
        let mem = self.mem_mb.last().copied().unwrap_or(0.0);
        format!("CPU {cpu:.1}%  ·  {mem:.0} MB")
    }
}

fn push(v: &mut Vec<f32>, x: f32) {
    if v.len() >= SAMPLES {
        v.remove(0);
    }
    v.push(x);
}

/// SVG path for a sparkline in a 100 x 30 viewbox. `max` scales the top.
pub fn sparkline(values: &[f32], max: f32) -> String {
    if values.is_empty() {
        return String::new();
    }
    let max = max.max(f32::EPSILON);
    let step = 100.0 / (SAMPLES - 1) as f32;
    let x0 = 100.0 - step * (values.len() - 1) as f32;
    let mut d = String::new();
    for (i, v) in values.iter().enumerate() {
        let x = x0 + step * i as f32;
        let y = 30.0 - (v / max).clamp(0.0, 1.0) * 28.0;
        d.push_str(&format!("{}{x:.2} {y:.2} ", if i == 0 { "M" } else { "L" }));
    }
    d
}

/// (user+kernel CPU seconds, resident memory bytes)
#[cfg(windows)]
fn read_process() -> (Option<f64>, Option<u64>) {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    // SAFETY: plain Win32 queries on the current process with correctly sized out-parameters.
    unsafe {
        let h = GetCurrentProcess();
        let zero = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
        let (mut c, mut e, mut k, mut u) = (zero, zero, zero, zero);
        let cpu = (GetProcessTimes(h, &mut c, &mut e, &mut k, &mut u) != 0).then(|| {
            let t = |f: FILETIME| ((f.dwHighDateTime as u64) << 32 | f.dwLowDateTime as u64) as f64 / 1e7;
            t(k) + t(u)
        });
        let mut pmc: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
        pmc.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
        let mem = (GetProcessMemoryInfo(h, &mut pmc, pmc.cb) != 0).then_some(pmc.WorkingSetSize as u64);
        (cpu, mem)
    }
}

#[cfg(target_os = "linux")]
fn read_process() -> (Option<f64>, Option<u64>) {
    let cpu = std::fs::read_to_string("/proc/self/stat").ok().and_then(|s| {
        // fields after the ")" of the command name; utime and stime are fields 14 and 15
        let rest = s.rsplit_once(')')?.1;
        let f: Vec<&str> = rest.split_whitespace().collect();
        let ticks = f.get(11)?.parse::<f64>().ok()? + f.get(12)?.parse::<f64>().ok()?;
        Some(ticks / 100.0)
    });
    let mem = std::fs::read_to_string("/proc/self/statm")
        .ok()
        .and_then(|s| s.split_whitespace().nth(1)?.parse::<u64>().ok())
        .map(|pages| pages * 4096);
    (cpu, mem)
}

#[cfg(not(any(windows, target_os = "linux")))]
fn read_process() -> (Option<f64>, Option<u64>) {
    (None, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_and_sparkline() {
        let mut s = Stats::new();
        s.sample();
        s.sample();
        assert!(!s.mem_mb.is_empty());
        assert!(s.label().contains("MB"));
        let d = sparkline(&[0.0, 50.0, 100.0], 100.0);
        assert!(d.starts_with('M') && d.trim_end().ends_with("2.00"));
    }
}
