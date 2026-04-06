use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static LAST_SQL_LATENCY_MS: AtomicU64 = AtomicU64::new(0);
static LAST_MCP_LATENCY_MS: AtomicU64 = AtomicU64::new(0);
static LAST_SAMPLE_AT_MS: AtomicU64 = AtomicU64::new(0);
static LAST_DEGRADED_AT_MS: AtomicU64 = AtomicU64::new(0);

const SERVICE_SAMPLE_TTL_MS: u64 = 5_000;
const SERVICE_RECOVERY_WINDOW_MS: u64 = 15_000;

#[derive(Clone, Copy)]
pub enum ServiceKind {
    Sql,
    Mcp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServicePressure {
    Healthy,
    Recovering,
    Degraded,
    Critical,
}

pub fn record_latency(kind: ServiceKind, latency_ms: u64) {
    let now = now_ms();
    match kind {
        ServiceKind::Sql => LAST_SQL_LATENCY_MS.store(latency_ms, Ordering::Relaxed),
        ServiceKind::Mcp => LAST_MCP_LATENCY_MS.store(latency_ms, Ordering::Relaxed),
    }
    if latency_ms >= 500 {
        LAST_DEGRADED_AT_MS.store(now, Ordering::Relaxed);
    }
    LAST_SAMPLE_AT_MS.store(now, Ordering::Relaxed);
}

pub fn recent_peak_latency_ms() -> u64 {
    recent_peak_latency_ms_at(now_ms())
}

pub fn current_pressure() -> ServicePressure {
    current_pressure_at(now_ms())
}

fn recent_peak_latency_ms_at(now_ms: u64) -> u64 {
    let last_seen = LAST_SAMPLE_AT_MS.load(Ordering::Relaxed);
    if last_seen == 0 || now_ms.saturating_sub(last_seen) > SERVICE_SAMPLE_TTL_MS {
        return 0;
    }

    LAST_SQL_LATENCY_MS
        .load(Ordering::Relaxed)
        .max(LAST_MCP_LATENCY_MS.load(Ordering::Relaxed))
}

fn current_pressure_at(now_ms: u64) -> ServicePressure {
    let last_seen = LAST_SAMPLE_AT_MS.load(Ordering::Relaxed);
    let last_degraded = LAST_DEGRADED_AT_MS.load(Ordering::Relaxed);
    if last_seen == 0 {
        return ServicePressure::Healthy;
    }

    let age_ms = now_ms.saturating_sub(last_seen);
    let peak = LAST_SQL_LATENCY_MS
        .load(Ordering::Relaxed)
        .max(LAST_MCP_LATENCY_MS.load(Ordering::Relaxed));

    if age_ms <= SERVICE_SAMPLE_TTL_MS {
        if peak >= 1_500 {
            ServicePressure::Critical
        } else if peak >= 500 {
            ServicePressure::Degraded
        } else if last_degraded != 0
            && now_ms.saturating_sub(last_degraded) <= SERVICE_RECOVERY_WINDOW_MS
        {
            ServicePressure::Recovering
        } else {
            ServicePressure::Healthy
        }
    } else if last_degraded != 0
        && now_ms.saturating_sub(last_degraded) <= SERVICE_RECOVERY_WINDOW_MS
    {
        if peak >= 500 || age_ms <= SERVICE_SAMPLE_TTL_MS * 3 {
            ServicePressure::Recovering
        } else {
            ServicePressure::Healthy
        }
    } else {
        ServicePressure::Healthy
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static TEST_GUARD: Mutex<()> = Mutex::new(());

    #[test]
    fn test_recent_peak_latency_expires() {
        let _guard = TEST_GUARD.lock().unwrap();
        LAST_SQL_LATENCY_MS.store(900, Ordering::Relaxed);
        LAST_MCP_LATENCY_MS.store(200, Ordering::Relaxed);
        LAST_SAMPLE_AT_MS.store(10_000, Ordering::Relaxed);
        LAST_DEGRADED_AT_MS.store(10_000, Ordering::Relaxed);

        assert_eq!(recent_peak_latency_ms_at(12_000), 900);
        assert_eq!(recent_peak_latency_ms_at(16_000), 0);
    }

    #[test]
    fn test_recent_peak_latency_uses_max_surface() {
        let _guard = TEST_GUARD.lock().unwrap();
        LAST_SQL_LATENCY_MS.store(250, Ordering::Relaxed);
        LAST_MCP_LATENCY_MS.store(700, Ordering::Relaxed);
        LAST_SAMPLE_AT_MS.store(20_000, Ordering::Relaxed);
        LAST_DEGRADED_AT_MS.store(20_000, Ordering::Relaxed);

        assert_eq!(recent_peak_latency_ms_at(21_000), 700);
    }

    #[test]
    fn test_current_pressure_reports_critical_when_sample_is_fresh() {
        let _guard = TEST_GUARD.lock().unwrap();
        LAST_SQL_LATENCY_MS.store(1_700, Ordering::Relaxed);
        LAST_MCP_LATENCY_MS.store(200, Ordering::Relaxed);
        LAST_SAMPLE_AT_MS.store(30_000, Ordering::Relaxed);
        LAST_DEGRADED_AT_MS.store(30_000, Ordering::Relaxed);

        assert_eq!(current_pressure_at(31_000), ServicePressure::Critical);
    }

    #[test]
    fn test_current_pressure_enters_recovering_after_ttl() {
        let _guard = TEST_GUARD.lock().unwrap();
        LAST_SQL_LATENCY_MS.store(1_700, Ordering::Relaxed);
        LAST_MCP_LATENCY_MS.store(200, Ordering::Relaxed);
        LAST_SAMPLE_AT_MS.store(40_000, Ordering::Relaxed);
        LAST_DEGRADED_AT_MS.store(40_000, Ordering::Relaxed);

        assert_eq!(current_pressure_at(46_000), ServicePressure::Recovering);
    }

    #[test]
    fn test_current_pressure_returns_healthy_after_recovery_window() {
        let _guard = TEST_GUARD.lock().unwrap();
        LAST_SQL_LATENCY_MS.store(1_700, Ordering::Relaxed);
        LAST_MCP_LATENCY_MS.store(200, Ordering::Relaxed);
        LAST_SAMPLE_AT_MS.store(50_000, Ordering::Relaxed);
        LAST_DEGRADED_AT_MS.store(50_000, Ordering::Relaxed);

        assert_eq!(current_pressure_at(66_000), ServicePressure::Healthy);
    }

    #[test]
    fn test_current_pressure_stays_recovering_after_low_latency_sample() {
        let _guard = TEST_GUARD.lock().unwrap();
        LAST_SQL_LATENCY_MS.store(120, Ordering::Relaxed);
        LAST_MCP_LATENCY_MS.store(140, Ordering::Relaxed);
        LAST_SAMPLE_AT_MS.store(70_000, Ordering::Relaxed);
        LAST_DEGRADED_AT_MS.store(68_000, Ordering::Relaxed);

        assert_eq!(current_pressure_at(71_000), ServicePressure::Recovering);
        assert_eq!(current_pressure_at(84_500), ServicePressure::Healthy);
    }
}
