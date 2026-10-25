//! The models this service will run. A call names a model by alias (or by its
//! full GenieX id); anything not in the catalog is refused, so a caller can
//! never make GenieX pull or load an arbitrary model.

use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Llm,
    /// Takes an inline image.
    Vlm,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Name callers use: `[a-z0-9._-]+`.
    pub alias: String,
    /// The id GenieX knows it by (`geniex list`).
    pub geniex_id: String,
    pub kind: Kind,
    /// Memory the model is expected to take once loaded, MiB. Replaced by
    /// the measured cost after the first load.
    pub est_mib: u64,
    /// Context window GenieX is started with for this model.
    pub nctx: u32,
}

#[derive(Debug, Clone)]
pub struct Catalog {
    entries: Vec<Entry>,
}

fn e(alias: &str, id: &str, kind: Kind, est_mib: u64, nctx: u32) -> Entry {
    Entry {
        alias: alias.into(),
        geniex_id: id.into(),
        kind,
        est_mib,
        nctx,
    }
}

impl Catalog {
    /// The models measured on the Ventuno Q (docs/research/ventuno-q-npu-bench.md).
    /// Gemma 4 E4B is left out on purpose: it is OOM-killed on the 14 GiB board.
    pub fn ventuno() -> Self {
        use Kind::{Llm, Vlm};
        Self {
            entries: vec![
                e(
                    "gemma3-1b",
                    "unsloth/gemma-3-1b-it-GGUF:Q4_0",
                    Llm,
                    1400,
                    4096,
                ),
                e(
                    "qwen3.5-0.8b",
                    "unsloth/Qwen3.5-0.8B-GGUF:Q4_0",
                    Llm,
                    1400,
                    4096,
                ),
                e(
                    "gemma4-e2b",
                    "google/gemma-4-E2B-it-qat-q4_0-gguf:Q4_0",
                    Llm,
                    6000,
                    4096,
                ),
                e(
                    "qwen3-4b",
                    "qualcomm/qwen3_4b_instruct_2507:W4A16",
                    Llm,
                    4500,
                    2048,
                ),
                e(
                    "qwen3-vl-4b",
                    "qualcomm/qwen3_vl_4b_instruct:W4A16",
                    Vlm,
                    6000,
                    2048,
                ),
                e(
                    "qwen2.5-vl-7b",
                    "qualcomm/qwen2_5_vl_7b_instruct:W4A16",
                    Vlm,
                    9000,
                    2048,
                ),
            ],
        }
    }

    /// `[{"alias","geniex_id","kind":"llm|vlm","est_mib","nctx"}, ...]`.
    pub fn from_json(text: &str) -> Result<Self, String> {
        let v: Value = serde_json::from_str(text).map_err(|e| format!("catalog: {e}"))?;
        let arr = v.as_array().ok_or("catalog must be a JSON array")?;
        let mut entries = Vec::new();
        for (i, m) in arr.iter().enumerate() {
            let s = |k: &str| {
                m.get(k)
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(String::from)
                    .ok_or(format!("catalog[{i}].{k} is required"))
            };
            let alias = s("alias")?;
            if !alias
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._-".contains(&b))
            {
                return Err(format!("catalog[{i}].alias {alias:?} is not [a-z0-9._-]"));
            }
            if entries.iter().any(|x: &Entry| x.alias == alias) {
                return Err(format!("catalog[{i}].alias {alias:?} repeats"));
            }
            let kind = match m.get("kind").and_then(Value::as_str).unwrap_or("llm") {
                "llm" => Kind::Llm,
                "vlm" => Kind::Vlm,
                other => return Err(format!("catalog[{i}].kind {other:?} is not llm or vlm")),
            };
            let est_mib = m
                .get("est_mib")
                .and_then(Value::as_u64)
                .ok_or(format!("catalog[{i}].est_mib is required"))?;
            let nctx = m
                .get("nctx")
                .and_then(Value::as_u64)
                .unwrap_or(4096)
                .clamp(512, 16384) as u32;
            entries.push(Entry {
                alias,
                geniex_id: s("geniex_id")?,
                kind,
                est_mib,
                nctx,
            });
        }
        if entries.is_empty() {
            return Err("catalog is empty".into());
        }
        Ok(Self { entries })
    }

    /// By alias, or by exact GenieX id.
    pub fn resolve(&self, name: &str) -> Option<&Entry> {
        self.entries
            .iter()
            .find(|x| x.alias == name)
            .or_else(|| self.entries.iter().find(|x| x.geniex_id == name))
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_aliases_and_ids_and_refuses_the_rest() {
        let c = Catalog::ventuno();
        assert_eq!(c.resolve("qwen3-4b").unwrap().kind, Kind::Llm);
        assert_eq!(
            c.resolve("unsloth/gemma-3-1b-it-GGUF:Q4_0").unwrap().alias,
            "gemma3-1b"
        );
        assert!(
            c.resolve("gemma4-e4b").is_none(),
            "E4B does not fit the board"
        );
        assert!(c
            .resolve("google/gemma-4-E4B-it-qat-q4_0-gguf:Q4_0")
            .is_none());
        assert!(c.resolve("../../etc/passwd").is_none());
    }

    #[test]
    fn a_catalog_file_is_checked() {
        let ok = r#"[{"alias":"a","geniex_id":"x/y:Q4_0","est_mib":100}]"#;
        assert_eq!(Catalog::from_json(ok).unwrap().entries().len(), 1);
        assert!(Catalog::from_json("[]").is_err());
        assert!(Catalog::from_json(r#"[{"alias":"A","geniex_id":"x","est_mib":1}]"#).is_err());
        assert!(Catalog::from_json(r#"[{"alias":"a","geniex_id":"x"}]"#).is_err());
        assert!(
            Catalog::from_json(r#"[{"alias":"a","geniex_id":"x","est_mib":1,"kind":"tts"}]"#)
                .is_err()
        );
        let dup = r#"[{"alias":"a","geniex_id":"x","est_mib":1},{"alias":"a","geniex_id":"y","est_mib":1}]"#;
        assert!(Catalog::from_json(dup).is_err());
    }
}
