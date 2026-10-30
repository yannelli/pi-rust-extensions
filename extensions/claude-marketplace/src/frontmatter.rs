/// A Markdown file with optional `---` frontmatter, edited line by line.
#[derive(Debug, Clone, PartialEq)]
pub struct Doc {
    pub front: Option<Vec<String>>,
    pub body: String,
}

impl Doc {
    pub fn parse(text: &str) -> Doc {
        let mut lines = text.split_inclusive('\n');
        let first = lines.next().unwrap_or("");
        if first.trim_end() != "---" {
            return Doc {
                front: None,
                body: text.to_owned(),
            };
        }
        let mut front = Vec::new();
        let mut consumed = first.len();
        for line in lines {
            consumed += line.len();
            if line.trim_end() == "---" {
                return Doc {
                    front: Some(front),
                    body: text[consumed..].to_owned(),
                };
            }
            front.push(line.trim_end_matches(['\n', '\r']).to_owned());
        }
        Doc {
            front: None,
            body: text.to_owned(),
        }
    }

    fn key_line(&self, key: &str) -> Option<usize> {
        let front = self.front.as_ref()?;
        front.iter().position(|line| {
            line.strip_prefix(key)
                .is_some_and(|rest| rest.trim_start().starts_with(':'))
        })
    }

    /// Indexes of the indented lines that continue the value at `index`.
    fn continuation(&self, index: usize) -> usize {
        let front = self.front.as_deref().unwrap_or_default();
        let mut end = index + 1;
        while end < front.len() && (front[end].starts_with([' ', '\t']) || front[end].is_empty()) {
            end += 1;
        }
        end
    }

    pub fn get(&self, key: &str) -> Option<String> {
        let index = self.key_line(key)?;
        let line = &self.front.as_ref()?[index];
        let value = line.split_once(':')?.1.trim();
        Some(unquote(value).to_owned())
    }

    /// A list value: `a, b`, `[a, b]`, or an indented `- a` block.
    pub fn get_list(&self, key: &str) -> Option<Vec<String>> {
        let index = self.key_line(key)?;
        let value = self.get(key)?;
        if value.is_empty() {
            let front = self.front.as_ref()?;
            let items = front[index + 1..self.continuation(index)]
                .iter()
                .filter_map(|line| line.trim().strip_prefix('-'))
                .map(|item| unquote(item.trim()).to_owned())
                .filter(|item| !item.is_empty())
                .collect();
            return Some(items);
        }
        let inner = value
            .strip_prefix('[')
            .and_then(|v| v.strip_suffix(']'))
            .unwrap_or(&value);
        Some(
            inner
                .split(',')
                .map(|item| unquote(item.trim()).to_owned())
                .filter(|item| !item.is_empty())
                .collect(),
        )
    }

    /// Replaces `key` and its indented continuation lines, or appends it.
    pub fn set(&mut self, key: &str, value: &str) {
        let line = format!("{key}: {value}");
        match self.key_line(key) {
            Some(index) => {
                let end = self.continuation(index);
                let front = self.front.get_or_insert_with(Vec::new);
                front.splice(index..end, [line]);
            }
            None => self.front.get_or_insert_with(Vec::new).push(line),
        }
    }

    pub fn render(&self) -> String {
        match &self.front {
            Some(front) => {
                let mut out = String::from("---\n");
                for line in front {
                    out.push_str(line);
                    out.push('\n');
                }
                out.push_str("---\n");
                out.push_str(&self.body);
                out
            }
            None => self.body.clone(),
        }
    }
}

fn unquote(value: &str) -> &str {
    for quote in ['"', '\''] {
        if let Some(inner) = value
            .strip_prefix(quote)
            .and_then(|v| v.strip_suffix(quote))
        {
            return inner;
        }
    }
    value
}

/// Replaces `${NAME}` tokens found in `vars` in one pass; other tokens stay.
pub fn substitute(text: &str, vars: &[(&str, String)]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let replaced = after.find('}').and_then(|end| {
            let name = &after[..end];
            let value = vars.iter().find(|(key, _)| *key == name)?;
            Some((value.1.as_str(), end))
        });
        match replaced {
            Some((value, end)) => {
                out.push_str(value);
                rest = &after[end + 1..];
            }
            None => {
                out.push_str("${");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_and_rewrites_frontmatter() {
        let text = "---\nname: old\ntools:\n  - Bash\n  - \"Read\"\nmodel: sonnet\n---\nBody\n";
        let mut doc = Doc::parse(text);
        assert_eq!(doc.get("name").as_deref(), Some("old"));
        assert_eq!(doc.get_list("tools").unwrap(), ["Bash", "Read"]);
        doc.set("tools", "bash, read");
        doc.set("name", "new");
        doc.set("color", "red");
        assert_eq!(
            doc.render(),
            "---\nname: new\ntools: bash, read\nmodel: sonnet\ncolor: red\n---\nBody\n"
        );
    }

    #[test]
    fn list_forms() {
        let doc = Doc::parse("---\na: x, y\nb: [x, 'y']\n---\n");
        assert_eq!(doc.get_list("a").unwrap(), ["x", "y"]);
        assert_eq!(doc.get_list("b").unwrap(), ["x", "y"]);
        assert!(Doc::parse("no front").front.is_none());
    }

    #[test]
    fn substitutes_known_tokens_once() {
        let vars = [("A", "${B}".to_owned()), ("B", "b".to_owned())];
        assert_eq!(substitute("x ${A} ${C} ${B} ${", &vars), "x ${B} ${C} b ${");
    }
}
