pub(super) fn parse_i64_field(value: &serde_json::Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_u64().map(|v| v.min(i64::MAX as u64) as i64))
        .or_else(|| value.as_str().and_then(|s| s.parse::<i64>().ok()))
}

pub(super) fn parse_u64_field(value: &serde_json::Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_i64().map(|v| v.max(0) as u64))
        .or_else(|| value.as_str().and_then(|s| s.parse::<u64>().ok()))
}
