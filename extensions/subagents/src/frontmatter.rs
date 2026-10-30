//! The YAML subset of Claude agent files: `key: value`, quoted strings, `[a, b]` and `- item`
//! lists, and `|` / `>` block scalars.

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FmValue {
    Text(String),
    List(Vec<String>),
}

impl FmValue {
    /// Lists join with `, `; text stays as is.
    pub fn as_text(&self) -> String {
        match self {
            FmValue::Text(text) => text.clone(),
            FmValue::List(items) => items.join(", "),
        }
    }

    /// Text splits on commas; empty items drop.
    pub fn as_list(&self) -> Vec<String> {
        let items = match self {
            FmValue::Text(text) => text.split(',').map(str::to_owned).collect(),
            FmValue::List(items) => items.clone(),
        };
        items
            .into_iter()
            .map(|item| item.trim().to_owned())
            .filter(|item| !item.is_empty())
            .collect()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Frontmatter {
    pub fields: Vec<(String, FmValue)>,
    pub body: String,
}

impl Frontmatter {
    pub fn parse(content: &str) -> Self {
        let content = content.trim_start_matches('\u{feff}').replace("\r\n", "\n");
        let Some(rest) = content.strip_prefix("---\n") else {
            return Self {
                fields: Vec::new(),
                body: content.trim().to_owned(),
            };
        };
        let after_line = |s: &str| s.split_once('\n').map_or("", |(_, body)| body).to_owned();
        let (head, body) = if rest.starts_with("---") {
            (String::new(), after_line(rest))
        } else {
            match rest.find("\n---") {
                Some(end) => (rest[..end].to_owned(), after_line(&rest[end + 1..])),
                None => (rest.to_owned(), String::new()),
            }
        };
        Self {
            fields: parse_fields(&head),
            body: body.trim().to_owned(),
        }
    }

    pub fn get(&self, key: &str) -> Option<&FmValue> {
        let wanted = normalize_key(key);
        self.fields
            .iter()
            .find(|(name, _)| normalize_key(name) == wanted)
            .map(|(_, value)| value)
    }

    pub fn text(&self, key: &str) -> Option<String> {
        self.get(key)
            .map(FmValue::as_text)
            .filter(|text| !text.is_empty())
    }

    /// Sets (`Some`) or removes (`None`) a field, keeping the field order.
    pub fn set(&mut self, key: &str, value: Option<FmValue>) {
        let wanted = normalize_key(key);
        let index = self
            .fields
            .iter()
            .position(|(name, _)| normalize_key(name) == wanted);
        match (index, value) {
            (Some(i), Some(value)) => self.fields[i].1 = value,
            (Some(i), None) => drop(self.fields.remove(i)),
            (None, Some(value)) => self.fields.push((key.to_owned(), value)),
            (None, None) => {}
        }
    }

    pub fn render(&self) -> String {
        let mut out = String::from("---\n");
        for (key, value) in &self.fields {
            let text = match value {
                FmValue::Text(text) => text.clone(),
                FmValue::List(items) => items.join(", "),
            };
            out.push_str(&format!("{key}: {}\n", quote(&text)));
        }
        out.push_str("---\n");
        if !self.body.is_empty() {
            out.push('\n');
            out.push_str(self.body.trim());
            out.push('\n');
        }
        out
    }
}

/// `maxTurns`, `max_turns`, and `max-turns` are the same key.
fn normalize_key(key: &str) -> String {
    key.chars()
        .filter(|c| *c != '_' && *c != '-')
        .flat_map(char::to_lowercase)
        .collect()
}

fn parse_fields(head: &str) -> Vec<(String, FmValue)> {
    let lines: Vec<&str> = head.lines().collect();
    let mut fields = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        i += 1;
        if line.trim().is_empty() || line.trim_start().starts_with('#') || line.starts_with(' ') {
            continue;
        }
        let Some((key, raw)) = line.split_once(':') else {
            continue;
        };
        let raw = strip_comment(raw.trim());
        let value = match raw {
            "|" | "|-" | ">" | ">-" => {
                let mut block = Vec::new();
                while i < lines.len() && (lines[i].starts_with(' ') || lines[i].trim().is_empty()) {
                    block.push(lines[i].trim());
                    i += 1;
                }
                let sep = if raw.starts_with('|') { "\n" } else { " " };
                FmValue::Text(block.join(sep).trim().to_owned())
            }
            "" => {
                let mut items = Vec::new();
                while i < lines.len() && lines[i].trim_start().starts_with("- ") {
                    items.push(unquote(lines[i].trim_start()[2..].trim()));
                    i += 1;
                }
                if items.is_empty() {
                    FmValue::Text(String::new())
                } else {
                    FmValue::List(items)
                }
            }
            raw if raw.starts_with('[') && raw.ends_with(']') => FmValue::List(
                raw[1..raw.len() - 1]
                    .split(',')
                    .map(|item| unquote(item.trim()))
                    .filter(|item| !item.is_empty())
                    .collect(),
            ),
            raw => FmValue::Text(unquote(raw)),
        };
        fields.push((key.trim().to_owned(), value));
    }
    fields
}

fn strip_comment(raw: &str) -> &str {
    if raw.starts_with('"') || raw.starts_with('\'') {
        return raw;
    }
    raw.find(" #").map_or(raw, |at| raw[..at].trim_end())
}

fn unquote(raw: &str) -> String {
    let raw = raw.trim();
    if raw.len() >= 2 && raw.starts_with('"') && raw.ends_with('"') {
        return serde_json::from_str(raw).unwrap_or_else(|_| raw[1..raw.len() - 1].to_owned());
    }
    if raw.len() >= 2 && raw.starts_with('\'') && raw.ends_with('\'') {
        return raw[1..raw.len() - 1].replace("''", "'");
    }
    raw.to_owned()
}

fn quote(text: &str) -> String {
    let plain = !text.is_empty()
        && !text.contains(": ")
        && !text.contains(" #")
        && !text.contains('\n')
        && !text.starts_with(|c: char| "\"'[{>|*&!%@`#-".contains(c) || c.is_whitespace())
        && !text.ends_with(char::is_whitespace);
    if plain {
        text.to_owned()
    } else {
        serde_json::to_string(text).unwrap_or_default()
    }
}
