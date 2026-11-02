//! `deferred_search` queries: `select:a,b` loads exact names; words rank by name and text.

use crate::catalog::{Body, Item};
use crate::config::Kind;

#[derive(Debug, Default, PartialEq)]
pub struct Query {
    pub select: Vec<String>,
    /// `+word`: every match must contain it.
    pub required: Vec<String>,
    pub terms: Vec<String>,
}

pub fn parse(query: &str) -> Query {
    let query = query.trim();
    if let Some(names) = query.strip_prefix("select:") {
        let select = names.split(',').map(str::trim).filter(|n| !n.is_empty());
        return Query {
            select: select.map(str::to_owned).collect(),
            ..Query::default()
        };
    }
    let mut parsed = Query::default();
    for word in query.split_whitespace() {
        match word.strip_prefix('+') {
            Some(word) if !word.is_empty() => parsed.required.push(word.to_lowercase()),
            Some(_) => {}
            None => parsed.terms.push(word.to_lowercase()),
        }
    }
    parsed
}

/// Items for `query`, best first, at most `max`.
pub fn find<'a>(items: &'a [Item], query: &Query, kind: Option<Kind>, max: usize) -> Vec<&'a Item> {
    let items: Vec<&Item> = items
        .iter()
        .filter(|i| kind.is_none_or(|k| i.kind == k))
        .collect();
    if !query.select.is_empty() {
        return select(&items, &query.select);
    }
    let mut scored: Vec<(usize, &Item)> = items
        .into_iter()
        .filter_map(|item| score(item, query).map(|s| (s, item)))
        .collect();
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| (a.1.kind, &a.1.name).cmp(&(b.1.kind, &b.1.name)))
    });
    scored.into_iter().take(max).map(|(_, item)| item).collect()
}

/// Exact names first, then a tool whose last `__` part is the name (`create_issue`).
fn select<'a>(items: &[&'a Item], names: &[String]) -> Vec<&'a Item> {
    let mut found: Vec<&Item> = Vec::new();
    for wanted in names {
        let exact = items.iter().find(|i| i.name.eq_ignore_ascii_case(wanted));
        let suffix = || {
            let tail = format!("__{}", wanted.to_lowercase());
            items
                .iter()
                .find(|i| i.name.to_lowercase().ends_with(&tail))
        };
        if let Some(item) = exact.or_else(suffix)
            && !found.iter().any(|f| std::ptr::eq(*f, *item))
        {
            found.push(item);
        }
    }
    found
}

/// Lowercase description and text, for search.
fn haystack(item: &Item) -> String {
    let mut text = item.description.to_lowercase();
    if let Body::Text(body) = &item.body {
        text.push('\n');
        text.push_str(&body.chars().take(20_000).collect::<String>().to_lowercase());
    }
    text
}

fn score(item: &Item, query: &Query) -> Option<usize> {
    let name = item.name.to_lowercase();
    let parts: Vec<&str> = name
        .split(|c: char| !c.is_alphanumeric())
        .filter(|p| !p.is_empty())
        .collect();
    let haystack = haystack(item);
    let term_score = |term: &str| {
        if name == term {
            10
        } else if parts.contains(&term) {
            5
        } else if name.contains(term) {
            3
        } else if haystack.contains(term) {
            1
        } else {
            0
        }
    };
    let mut total = 0;
    for term in &query.required {
        match term_score(term) {
            0 => return None,
            s => total += s,
        }
    }
    total += query.terms.iter().map(|t| term_score(t)).sum::<usize>();
    let browsing = query.required.is_empty() && query.terms.is_empty();
    (total > 0 || browsing).then_some(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Mode;

    fn item(kind: Kind, name: &str, description: &str) -> Item {
        let body = Body::Text(String::new());
        Item {
            kind,
            name: name.into(),
            description: description.into(),
            mode: Mode::Names,
            tokens: 1,
            body,
        }
    }

    fn names(found: Vec<&Item>) -> Vec<&str> {
        found.into_iter().map(|i| i.name.as_str()).collect()
    }

    #[test]
    fn parses_select_required_and_terms() {
        let q = parse(" select: a , b,, ");
        assert_eq!(q.select, ["a", "b"]);
        let q = parse("+Slack send message +");
        assert_eq!(
            (q.required, q.terms),
            (vec!["slack".into()], vec!["send".into(), "message".into()])
        );
    }

    #[test]
    fn ranks_name_hits_over_description_hits() {
        let items = vec![
            item(
                Kind::Tool,
                "mcp__slack__post_message",
                "Send a message to a channel",
            ),
            item(Kind::Tool, "mcp__mail__send", "Send email"),
            item(Kind::Skill, "notes", "Take a message"),
        ];
        assert_eq!(
            names(find(&items, &parse("send"), None, 5)),
            ["mcp__mail__send", "mcp__slack__post_message"]
        );
        assert_eq!(
            names(find(&items, &parse("+slack message"), None, 5)),
            ["mcp__slack__post_message"]
        );
        assert_eq!(
            names(find(&items, &parse("message"), Some(Kind::Skill), 5)),
            ["notes"]
        );
        assert_eq!(find(&items, &parse("message"), None, 1).len(), 1);
        assert!(find(&items, &parse("zzz"), None, 5).is_empty());
        assert_eq!(find(&items, &parse(""), None, 5).len(), 3);
    }

    #[test]
    fn select_takes_exact_then_tool_suffix() {
        let items = vec![
            item(Kind::Tool, "mcp__gh__create_issue", ""),
            item(Kind::Skill, "PDF", ""),
        ];
        let q = parse("select:pdf,create_issue,missing,PDF");
        assert_eq!(
            names(find(&items, &q, None, 1)),
            ["PDF", "mcp__gh__create_issue"]
        );
    }
}
