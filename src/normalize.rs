pub fn normalize_name(name: &str) -> String {
    deunicode::deunicode(name)
        .to_lowercase()
        .replace('-', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
