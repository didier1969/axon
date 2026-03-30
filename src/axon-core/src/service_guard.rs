use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static LAST_SQL_LATENCY_MS: AtomicU64 = AtomicU64::new(0);
static LAST_MCP_LATENCY_MS: AtomicU64 = AtomicU64::new(0);
static LAST_SAMPLE_AT_MS: AtomicU64 = AtomicU64::new(0);

const SERVICE_SAMPLE_TTL_MS: u64 = 5_000;

#[derive(Clone, Copy)]
pub enum ServiceKind {
    Sql,
    Mcp,
}

pub fn record_latency(kind: ServiceKind, latency_ms: u64) {
    let now = now_ms();
    match kind {
        ServiceKind::Sql => LAST_SQL_LATENCY_MS.store(latency_ms, Ordering::Relaxed),
        ServiceKind::Mcp => LAST_MCP_LATENCY_MS.store(latency_ms, Ordering::Relaxed),
    }
    LAST_SAMPLE_AT_MS.store(now, Ordering::Relaxed);
}

pub fn recent_peak_latency_ms() -> u64 {
    recent_peak_latency_ms_at(now_ms())
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

        assert_eq!(recent_peak_latency_ms_at(12_000), 900);
        assert_eq!(recent_peak_latency_ms_at(16_000), 0);
    }

    #[test]
    fn test_recent_peak_latency_uses_max_surface() {
        let _guard = TEST_GUARD.lock().unwrap();
        LAST_SQL_LATENCY_MS.store(250, Ordering::Relaxed);
        LAST_MCP_LATENCY_MS.store(700, Ordering::Relaxed);
        LAST_SAMPLE_AT_MS.store(20_000, Ordering::Relaxed);

        assert_eq!(recent_peak_latency_ms_at(21_000), 700);
    }
}
