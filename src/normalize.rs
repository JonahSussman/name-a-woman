pub fn normalize_name(name: &str) -> String {
    deunicode::deunicode(name).to_lowercase().trim().to_string()
}
