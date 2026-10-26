//! The cog's sensor guide (WeftOS ADR-104): `guide/guide.toml` plus one Markdown page per topic,
//! compiled in and served at `GET /guide` as `{"toml": "...", "pages": {id: md}}`.

pub const TOML: &str = include_str!("../guide/guide.toml");

pub const PAGES: [(&str, &str); 5] = [
    ("start", include_str!("../guide/start.md")),
    ("phrase", include_str!("../guide/phrase.md")),
    ("setup", include_str!("../guide/setup.md")),
    ("troubleshoot", include_str!("../guide/troubleshoot.md")),
    ("api", include_str!("../guide/api.md")),
];

pub fn bundle_json() -> serde_json::Value {
    let pages: serde_json::Map<String, serde_json::Value> = PAGES
        .iter()
        .map(|(id, md)| {
            (
                (*id).to_string(),
                serde_json::Value::String((*md).to_string()),
            )
        })
        .collect();
    serde_json::json!({ "toml": TOML, "pages": pages })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_pages_match_the_guide_toml_page_list() {
        let line = TOML
            .lines()
            .find(|l| l.starts_with("pages = ["))
            .expect("pages line");
        let listed: Vec<String> = line
            .trim_start_matches("pages = [")
            .trim_end_matches(']')
            .split(',')
            .map(|s| s.trim().trim_matches('"').to_string())
            .filter(|s| !s.is_empty())
            .collect();
        let have: Vec<String> = PAGES.iter().map(|(id, _)| id.to_string()).collect();
        assert_eq!(listed, have, "guide.toml pages must match the PAGES array");
    }
}
