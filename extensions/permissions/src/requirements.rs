//! Codex `requirements.toml`: the `[rules]` `prefix_rules` that admins enforce.

use toml_edit::{DocumentMut, Item, Value};

use crate::codex::{CodexRule, expand};
use crate::rules::Decision;

/// The rules of one `requirements.toml`; `allow` is refused, as in Codex.
pub fn parse_requirements(text: &str) -> Result<Vec<CodexRule>, String> {
    let doc: DocumentMut = text.parse().map_err(|e| format!("{e}"))?;
    let Some(rules) = doc.get("rules").and_then(|r| r.get("prefix_rules")) else {
        return Ok(Vec::new());
    };
    let entries: Vec<toml_edit::InlineTable> = match rules {
        Item::ArrayOfTables(tables) => tables
            .iter()
            .map(|t| t.clone().into_inline_table())
            .collect(),
        Item::Value(Value::Array(array)) => array
            .iter()
            .map(|v| {
                v.as_inline_table()
                    .cloned()
                    .ok_or("prefix_rules entries must be tables")
            })
            .collect::<Result<_, _>>()?,
        _ => return Err("rules.prefix_rules must be an array of tables".into()),
    };
    if entries.is_empty() {
        return Err("rules prefix_rules cannot be empty".into());
    }
    let mut out = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let tokens = entry
            .get("pattern")
            .and_then(Value::as_array)
            .ok_or(format!("prefix_rule {index} needs a pattern"))?;
        let alts: Vec<Vec<String>> = tokens
            .iter()
            .map(|token| token_alternatives(token).ok_or(format!("prefix_rule {index}: bad token")))
            .collect::<Result<_, _>>()?;
        if alts.is_empty() {
            return Err(format!("prefix_rule {index} has an empty pattern"));
        }
        let decision = match entry.get("decision").and_then(Value::as_str) {
            Some("prompt" | "prompt-user") => Decision::Ask,
            Some("forbidden") => Decision::Deny,
            Some("allow") => return Err(format!("prefix_rule {index}: allow is not permitted")),
            Some(other) => return Err(format!("prefix_rule {index}: unknown decision `{other}`")),
            None => return Err(format!("prefix_rule {index} is missing a decision")),
        };
        let justification = entry.get("justification").and_then(Value::as_str);
        if justification.is_some_and(|j| j.trim().is_empty()) {
            return Err(format!("prefix_rule {index} has an empty justification"));
        }
        let text = format!("requirements prefix_rules[{index}] {alts:?}");
        out.extend(expand(
            &alts,
            decision,
            justification.map(str::to_owned),
            &text,
        ));
    }
    Ok(out)
}

/// `{ token = "x" }` or `{ any_of = ["a", "b"] }`.
fn token_alternatives(token: &Value) -> Option<Vec<String>> {
    let table = token.as_inline_table()?;
    if let Some(single) = table.get("token").and_then(Value::as_str) {
        return Some(vec![single.to_owned()]);
    }
    let any = table.get("any_of")?.as_array()?;
    let alts: Vec<String> = any
        .iter()
        .map(|v| v.as_str().map(str::to_owned))
        .collect::<Option<_>>()?;
    (!alts.is_empty()).then_some(alts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_prefix_rules_and_refuses_allow() {
        let text = r#"
[rules]
prefix_rules = [
  { pattern = [{ token = "rm" }, { any_of = ["-rf", "-fr"] }], decision = "forbidden", justification = "no" },
  { pattern = [{ token = "git" }, { token = "push" }], decision = "prompt-user" },
]
"#;
        let rules = parse_requirements(text).unwrap();
        assert_eq!(rules.len(), 2);
        assert_eq!(rules[0].decision, Decision::Deny);
        assert!(
            rules[0]
                .rule
                .matches(&["rm".into(), "-fr".into(), "x".into()])
        );
        assert_eq!(rules[1].decision, Decision::Ask);
        let allow = r#"[rules]
prefix_rules = [{ pattern = [{ token = "ls" }], decision = "allow" }]"#;
        assert!(parse_requirements(allow).unwrap_err().contains("allow"));
        assert!(parse_requirements("other = 1").unwrap().is_empty());
        let tables =
            "[[rules.prefix_rules]]\npattern = [{ token = \"curl\" }]\ndecision = \"forbidden\"\n";
        assert_eq!(parse_requirements(tables).unwrap().len(), 1);
    }
}
