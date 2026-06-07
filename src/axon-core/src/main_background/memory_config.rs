pub(super) fn parse_rss_from_statm(content: &str) -> Option<u64> {
    content
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u64>().ok())
}

pub(super) fn current_rss_bytes() -> Option<u64> {
    let page_size = 4096;
    let content = std::fs::read_to_string("/proc/self/statm").ok()?;
    let rss_pages = parse_rss_from_statm(&content)?;
    Some(rss_pages * page_size)
}

pub(super) fn memory_reclaimer_enabled() -> bool {
    std::env::var("AXON_ENABLE_MEMORY_RECLAIMER")
        .ok()
        .map(|value| {
            !matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "off"
            )
        })
        .unwrap_or(true)
}

pub(super) fn memory_reclaimer_min_anon_bytes() -> u64 {
    std::env::var("AXON_MEMORY_RECLAIMER_MIN_ANON_MB")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(|mb| mb.saturating_mul(1024 * 1024))
        .unwrap_or(4 * 1024 * 1024 * 1024)
}

pub(super) fn memory_limit_bytes() -> u64 {
    let gb = std::env::var("AXON_MEMORY_LIMIT_GB")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|v| *v >= 2)
        .unwrap_or(14);
    gb * 1024 * 1024 * 1024
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statm_parser_reads_resident_pages() {
        assert_eq!(parse_rss_from_statm("100 42 8 0 0 0 0"), Some(42));
    }

    #[test]
    fn statm_parser_rejects_missing_resident_pages() {
        assert_eq!(parse_rss_from_statm("100"), None);
    }
}
