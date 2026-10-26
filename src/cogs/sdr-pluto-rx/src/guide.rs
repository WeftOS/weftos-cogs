//! The cog's sensor guide (WeftOS ADR-104): `guide/guide.toml` plus one Markdown page per topic,
//! compiled in and served at `GET /guide` as `{"toml": "...", "pages": {id: md}}`.

pub const TOML: &str = include_str!("../guide/guide.toml");

pub const PAGES: [(&str, &str); 6] = [
    ("start", include_str!("../guide/start.md")),
    ("connect", include_str!("../guide/connect.md")),
    ("range", include_str!("../guide/range.md")),
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
        let listed: Vec<&str> = line
            .trim_start_matches("pages = [")
            .trim_end_matches(']')
            .split(',')
            .map(|s| s.trim().trim_matches('"'))
            .collect();
        let embedded: Vec<&str> = PAGES.iter().map(|(id, _)| *id).collect();
        assert_eq!(listed, embedded);
    }

    #[test]
    fn every_page_starts_with_a_title_and_summary() {
        for (id, md) in PAGES {
            assert!(
                md.lines().next().is_some_and(|l| l.starts_with("# ")),
                "{id}: first line must be '# Title'"
            );
            assert!(
                md.lines().any(|l| l.starts_with("> ")),
                "{id}: needs a '> summary' line"
            );
        }
    }

    #[test]
    fn the_guide_states_the_declared_and_the_rated_range() {
        let range = PAGES.iter().find(|(id, _)| *id == "range").unwrap().1;
        assert!(range.contains("70 to 6000 MHz") && range.contains("325 to 3800 MHz"));
        let connect = PAGES.iter().find(|(id, _)| *id == "connect").unwrap().1;
        for word in ["orientation", "Power", "Antenna", "Heat"] {
            assert!(connect.contains(word), "connect page lacks {word}");
        }
    }
}
