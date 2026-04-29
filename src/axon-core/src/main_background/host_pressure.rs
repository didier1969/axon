use std::sync::{LazyLock, Mutex};

static HOST_PRESSURE_SAMPLER: LazyLock<Mutex<HostPressureSampler>> =
    LazyLock::new(|| Mutex::new(HostPressureSampler::default()));

#[derive(Debug, Clone, Copy, Default)]
pub(super) struct HostPressureSnapshot {
    pub(super) cpu_load: f64,
    pub(super) ram_load: f64,
    pub(super) io_wait: f64,
}

#[derive(Debug, Clone, Copy)]
struct ProcStatSample {
    total: u64,
    idle: u64,
    iowait: u64,
}

#[derive(Debug, Default)]
struct HostPressureSampler {
    previous: Option<ProcStatSample>,
}

pub(super) fn sample_host_pressure() -> HostPressureSnapshot {
    let cpu_sample = read_proc_stat_sample();
    let ram_load = read_ram_load_percent();

    match HOST_PRESSURE_SAMPLER.lock() {
        Ok(mut sampler) => {
            let previous = sampler.previous;
            sampler.previous = cpu_sample;

            let (cpu_load, io_wait) = match (previous, cpu_sample) {
                (Some(previous), Some(current)) => compute_cpu_and_io_percent(previous, current),
                _ => (0.0, 0.0),
            };

            HostPressureSnapshot {
                cpu_load,
                ram_load,
                io_wait,
            }
        }
        Err(_) => HostPressureSnapshot {
            cpu_load: 0.0,
            ram_load,
            io_wait: 0.0,
        },
    }
}

fn read_proc_stat_sample() -> Option<ProcStatSample> {
    let content = std::fs::read_to_string("/proc/stat").ok()?;
    parse_proc_stat_sample(&content)
}

fn parse_proc_stat_sample(content: &str) -> Option<ProcStatSample> {
    let line = content.lines().find(|line| line.starts_with("cpu "))?;
    let mut values = line.split_whitespace().skip(1);
    let user = values.next()?.parse::<u64>().ok()?;
    let nice = values.next()?.parse::<u64>().ok()?;
    let system = values.next()?.parse::<u64>().ok()?;
    let idle = values.next()?.parse::<u64>().ok()?;
    let iowait = values.next()?.parse::<u64>().ok()?;
    let irq = values
        .next()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    let softirq = values
        .next()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    let steal = values
        .next()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    let total = user + nice + system + idle + iowait + irq + softirq + steal;

    Some(ProcStatSample {
        total,
        idle,
        iowait,
    })
}

fn compute_cpu_and_io_percent(previous: ProcStatSample, current: ProcStatSample) -> (f64, f64) {
    let total_delta = current.total.saturating_sub(previous.total);
    if total_delta == 0 {
        return (0.0, 0.0);
    }

    let idle_delta = current.idle.saturating_sub(previous.idle);
    let iowait_delta = current.iowait.saturating_sub(previous.iowait);
    let busy_delta = total_delta.saturating_sub(idle_delta);
    let cpu_load = ((busy_delta as f64) / (total_delta as f64) * 100.0).clamp(0.0, 100.0);
    let io_wait = ((iowait_delta as f64) / (total_delta as f64) * 100.0).clamp(0.0, 100.0);

    (cpu_load, io_wait)
}

fn read_ram_load_percent() -> f64 {
    let content = match std::fs::read_to_string("/proc/meminfo") {
        Ok(content) => content,
        Err(_) => return 0.0,
    };
    parse_ram_load_percent(&content)
}

fn parse_ram_load_percent(content: &str) -> f64 {
    let mut total_kb = None;
    let mut available_kb = None;
    let mut free_kb = None;
    let mut buffers_kb = None;
    let mut cached_kb = None;

    for line in content.lines() {
        let mut parts = line.split_whitespace();
        let key = parts.next().unwrap_or_default();
        let value = parts
            .next()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0);

        match key {
            "MemTotal:" => total_kb = Some(value),
            "MemAvailable:" => available_kb = Some(value),
            "MemFree:" => free_kb = Some(value),
            "Buffers:" => buffers_kb = Some(value),
            "Cached:" => cached_kb = Some(value),
            _ => {}
        }
    }

    let total_kb = total_kb.unwrap_or(0);
    if total_kb == 0 {
        return 0.0;
    }

    let available_kb = available_kb
        .unwrap_or(free_kb.unwrap_or(0) + buffers_kb.unwrap_or(0) + cached_kb.unwrap_or(0));
    let used_kb = total_kb.saturating_sub(available_kb);

    ((used_kb as f64) / (total_kb as f64) * 100.0).clamp(0.0, 100.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proc_stat_sample_parses_cpu_totals() {
        let sample =
            parse_proc_stat_sample("cpu  10 2 3 100 5 7 11 13 0 0\ncpu0 1 1 1 1 1 1 1 1\n")
                .expect("cpu aggregate line should parse");

        assert_eq!(sample.total, 151);
        assert_eq!(sample.idle, 100);
        assert_eq!(sample.iowait, 5);
    }

    #[test]
    fn cpu_and_io_percent_uses_saturating_deltas() {
        let previous = ProcStatSample {
            total: 100,
            idle: 40,
            iowait: 5,
        };
        let current = ProcStatSample {
            total: 200,
            idle: 70,
            iowait: 15,
        };

        let (cpu_load, io_wait) = compute_cpu_and_io_percent(previous, current);

        assert_eq!(cpu_load, 70.0);
        assert_eq!(io_wait, 10.0);
    }

    #[test]
    fn ram_load_prefers_mem_available() {
        let load = parse_ram_load_percent(
            "MemTotal:       1000 kB\nMemFree:         100 kB\nBuffers:         100 kB\nCached:          100 kB\nMemAvailable:    250 kB\n",
        );

        assert_eq!(load, 75.0);
    }

    #[test]
    fn ram_load_falls_back_to_free_buffers_cached() {
        let load = parse_ram_load_percent(
            "MemTotal:       1000 kB\nMemFree:         100 kB\nBuffers:         100 kB\nCached:          200 kB\n",
        );

        assert_eq!(load, 60.0);
    }
}
