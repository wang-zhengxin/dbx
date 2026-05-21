use std::collections::BTreeSet;

use font_kit::source::SystemSource;

pub fn normalize_font_families(families: Vec<String>) -> Vec<String> {
    families
        .into_iter()
        .map(|family| family.trim().to_string())
        .filter(|family| !family.is_empty())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

#[tauri::command]
pub fn list_system_fonts() -> Result<Vec<String>, String> {
    SystemSource::new().all_families().map(normalize_font_families).map_err(|err| err.to_string())
}

#[cfg(test)]
mod tests {
    use super::normalize_font_families;

    #[test]
    fn normalizes_and_sorts_font_family_names() {
        assert_eq!(
            normalize_font_families(vec![
                "Maple Mono NF CN".to_string(),
                " ".to_string(),
                "Arial".to_string(),
                "Maple Mono NF CN".to_string(),
            ]),
            vec!["Arial".to_string(), "Maple Mono NF CN".to_string()],
        );
    }
}
