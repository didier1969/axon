pub fn is_valid_project_code(value: &str) -> bool {
    value.len() == 3 && value.chars().all(|ch| ch.is_ascii_alphanumeric())
}
