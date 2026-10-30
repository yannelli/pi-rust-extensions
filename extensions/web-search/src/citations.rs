//! Inserts `[n]` citation markers into answer text.

use serde_json::Value;

use crate::results::{Source, push_unique_source};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Citation {
    pub title: String,
    pub url: String,
    pub end_index: Option<usize>,
    pub cited_text: Option<String>,
}

fn insert_markers(text: &str, mut insertions: Vec<(usize, String)>) -> String {
    insertions.sort_by_key(|item| std::cmp::Reverse(item.0));
    insertions.dedup();
    let mut result = text.to_owned();
    for (index, marker) in insertions {
        let mut index = index.min(result.len());
        while !result.is_char_boundary(index) {
            index -= 1;
        }
        result.insert_str(index, &marker);
    }
    result
}

fn char_to_byte(text: &str, chars: usize) -> usize {
    text.char_indices()
        .nth(chars)
        .map_or(text.len(), |(byte, _)| byte)
}

/// OpenAI annotations: `end_index` counts characters of the answer.
pub fn apply_index_citations(text: &str, citations: &[Citation]) -> (String, Vec<Source>) {
    let mut sources = Vec::new();
    let mut insertions = Vec::new();
    for citation in citations.iter().filter(|c| !c.url.is_empty()) {
        if let Some(end) = citation.end_index {
            let number = push_unique_source(&mut sources, &citation.title, &citation.url) + 1;
            insertions.push((char_to_byte(text, end), format!("[{number}]")));
        }
    }
    for citation in citations.iter().filter(|c| !c.url.is_empty()) {
        push_unique_source(&mut sources, &citation.title, &citation.url);
    }
    (insert_markers(text, insertions), sources)
}

/// Grok and other inline-citation dialects keep the text as is.
pub fn preserve_inline_citations(text: &str, citations: &[Citation]) -> (String, Vec<Source>) {
    let mut sources = Vec::new();
    for citation in citations.iter().filter(|c| !c.url.is_empty()) {
        push_unique_source(&mut sources, &citation.title, &citation.url);
    }
    (text.to_owned(), sources)
}

/// Anthropic citations: the marker goes after the first match of the cited text.
pub fn apply_text_citations(text: &str, citations: &[Citation]) -> (String, Vec<Source>) {
    let mut sources = Vec::new();
    let mut insertions = Vec::new();
    for citation in citations.iter().filter(|c| !c.url.is_empty()) {
        let number = push_unique_source(&mut sources, &citation.title, &citation.url) + 1;
        let Some(cited) = citation.cited_text.as_deref().map(str::trim) else {
            continue;
        };
        if cited.is_empty() {
            continue;
        }
        if let Some(index) = text.find(cited) {
            insertions.push((index + cited.len(), format!("[{number}]")));
        }
    }
    (insert_markers(text, insertions), sources)
}

/// Gemini grounding: `segment.endIndex` counts UTF-8 bytes.
pub fn apply_grounding(text: &str, grounding: Option<&Value>) -> (String, Vec<Source>) {
    let empty = Vec::new();
    let list = |key: &str| grounding.and_then(|g| g[key].as_array()).unwrap_or(&empty);
    let (chunks, supports) = (list("groundingChunks"), list("groundingSupports"));
    let sources: Vec<Source> = chunks
        .iter()
        .filter(|chunk| chunk["web"].is_object())
        .map(|chunk| Source {
            title: chunk["web"]["title"]
                .as_str()
                .unwrap_or("Unknown")
                .to_owned(),
            url: chunk["web"]["uri"].as_str().unwrap_or_default().to_owned(),
        })
        .collect();
    if supports.is_empty() || sources.is_empty() {
        return (text.to_owned(), sources);
    }
    let insertions = supports
        .iter()
        .filter_map(|support| {
            let end = support["segment"]["endIndex"].as_u64()?;
            let indices = support["groundingChunkIndices"].as_array()?;
            let marker: String = indices
                .iter()
                .filter_map(Value::as_u64)
                .map(|i| format!("[{}]", i + 1))
                .collect();
            (!marker.is_empty()).then_some((end as usize, marker))
        })
        .collect();
    (insert_markers(text, insertions), sources)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn cite(url: &str, end: Option<usize>, cited: Option<&str>) -> Citation {
        Citation {
            title: url.into(),
            url: url.into(),
            end_index: end,
            cited_text: cited.map(Into::into),
        }
    }

    #[test]
    fn index_citations_count_characters() {
        let (text, sources) = apply_index_citations("héllo world", &[cite("u1", Some(5), None)]);
        assert_eq!(text, "héllo[1] world");
        assert_eq!(sources.len(), 1);
    }

    #[test]
    fn text_citations_follow_cited_text() {
        let citations = [
            cite("u1", None, Some("world")),
            cite("u2", None, Some("absent")),
        ];
        let (text, sources) = apply_text_citations("hello world.", &citations);
        assert_eq!(text, "hello world[1].");
        assert_eq!(sources.len(), 2);
    }

    #[test]
    fn grounding_inserts_byte_offsets() {
        let grounding = json!({
            "groundingChunks": [{ "web": { "uri": "https://a.com", "title": "A" } }],
            "groundingSupports": [{ "segment": { "endIndex": 6 }, "groundingChunkIndices": [0] }]
        });
        let (text, sources) = apply_grounding("é bcd efg", Some(&grounding));
        assert_eq!(text, "é bcd[1] efg");
        assert_eq!(sources[0].title, "A");
    }
}
