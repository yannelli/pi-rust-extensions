use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub fn center(&self) -> (f64, f64) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Node {
    #[serde(rename = "ref")]
    pub reference: String,
    pub role: String,
    pub label: Option<String>,
    pub value: Option<String>,
    pub identifier: Option<String>,
    pub frame: Option<Rect>,
    pub actions: Vec<String>,
    pub can_press: bool,
    pub can_set_value: bool,
    pub can_focus: bool,
    pub can_scroll: bool,
    pub focused: bool,
    pub picture_only: bool,
    pub path: Vec<usize>,
    pub children: Vec<Node>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Outline {
    pub look_id: String,
    pub root: Node,
}

/// `AXTextField` becomes `text_field`; already normalized roles stay.
pub fn normalize_role(raw: &str) -> String {
    let trimmed = raw.strip_prefix("AX").unwrap_or(raw);
    let mut out = String::new();
    for (index, ch) in trimmed.chars().enumerate() {
        if ch.is_uppercase() && index > 0 {
            out.push('_');
        }
        out.extend(ch.to_lowercase());
    }
    out
}

impl Node {
    pub fn assign_refs(&mut self, next: &mut usize) {
        *next += 1;
        self.reference = format!("@e{next}");
        for child in &mut self.children {
            child.assign_refs(next);
        }
    }

    pub fn find(&self, reference: &str) -> Option<&Node> {
        if self.reference == reference {
            return Some(self);
        }
        self.children.iter().find_map(|child| child.find(reference))
    }

    pub fn capabilities(&self) -> Vec<&'static str> {
        let flags = [
            (self.can_set_value, "setValue"),
            (self.can_press, "press"),
            (self.can_focus, "focus"),
            (self.can_scroll, "scroll"),
        ];
        flags.iter().filter(|f| f.0).map(|f| f.1).collect()
    }

    pub fn text(&self) -> String {
        let parts = [self.label.as_deref(), self.value.as_deref()];
        let parts: Vec<&str> = parts
            .into_iter()
            .flatten()
            .filter(|s| !s.is_empty())
            .collect();
        parts.join("\n")
    }

    pub fn line(&self) -> String {
        let label = self.label.as_deref().filter(|l| !l.is_empty());
        let mut line = format!(
            "{} {} {}",
            self.reference,
            self.role,
            serde_json::to_string(label.unwrap_or("(unlabeled)")).unwrap_or_default()
        );
        if let Some(value) = self.value.as_deref().filter(|v| !v.is_empty()) {
            line.push_str(&format!(
                " value={}",
                serde_json::to_string(&clip(value, 80)).unwrap_or_default()
            ));
        }
        let caps = self.capabilities();
        if !caps.is_empty() {
            line.push_str(&format!(" [{}]", caps.join(",")));
        }
        if self.focused {
            line.push_str(" (focused)");
        }
        if self.picture_only {
            line.push_str(" (pictureOnly)");
        }
        line
    }

    fn is_noise(&self) -> bool {
        self.label.as_deref().is_none_or(str::is_empty)
            && self.value.as_deref().is_none_or(str::is_empty)
            && self.capabilities().is_empty()
            && self.children.len() == 1
    }

    pub fn render(&self, max_depth: usize, max_lines: usize) -> (String, usize) {
        let mut lines = Vec::new();
        let mut omitted = 0;
        self.render_into(0, max_depth, max_lines, &mut lines, &mut omitted);
        (lines.join("\n"), omitted)
    }

    fn render_into(
        &self,
        depth: usize,
        max: usize,
        cap: usize,
        out: &mut Vec<String>,
        omitted: &mut usize,
    ) {
        if self.is_noise() && depth > 0 {
            return self.children[0].render_into(depth, max, cap, out, omitted);
        }
        if out.len() >= cap {
            *omitted += self.count();
            return;
        }
        out.push(format!("{}{}", "  ".repeat(depth), self.line()));
        if depth >= max {
            let hidden = self.count() - 1;
            if hidden > 0 {
                out.push(format!(
                    "{}  … {hidden} more (expand_ui {})",
                    "  ".repeat(depth),
                    self.reference
                ));
            }
            return;
        }
        for child in &self.children {
            child.render_into(depth + 1, max, cap, out, omitted);
        }
    }

    pub fn count(&self) -> usize {
        1 + self.children.iter().map(Node::count).sum::<usize>()
    }

    fn walk<'a>(&'a self, trail: &mut Vec<&'a str>, visit: &mut dyn FnMut(&'a Node, String)) {
        visit(self, trail.join(" > "));
        trail.push(&self.role);
        for child in &self.children {
            child.walk(trail, visit);
        }
        trail.pop();
    }
}

pub fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(max).collect();
    out.push('…');
    out
}

#[derive(Debug, Clone, PartialEq)]
pub struct Match<'a> {
    pub node: &'a Node,
    pub path: String,
    pub reason: &'static str,
    pub score: f64,
}

fn fuzzy(query: &str, candidate: &str) -> f64 {
    let words: Vec<&str> = query.split_whitespace().collect();
    if words.is_empty() {
        return 0.0;
    }
    let hits = words.iter().filter(|w| candidate.contains(*w)).count();
    hits as f64 / words.len() as f64
}

fn text_rank(node: &Node, query: &str) -> Option<(&'static str, f64)> {
    let haystacks = [
        node.label.as_deref(),
        node.value.as_deref(),
        node.identifier.as_deref(),
    ];
    let mut best: Option<(&'static str, f64)> = None;
    for hay in haystacks.into_iter().flatten() {
        let hay = hay.to_lowercase();
        let rank = if hay == query {
            Some(("exact", 4.0))
        } else if hay.starts_with(query) {
            Some(("prefix", 3.0))
        } else if hay.contains(query) {
            Some(("substring", 2.0))
        } else {
            let score = fuzzy(query, &hay);
            (score >= 0.5).then_some(("fuzzy", score))
        };
        if rank.is_some_and(|r| best.is_none_or(|b| r.1 > b.1)) {
            best = rank;
        }
    }
    best
}

/// Ranked search; returns the top `limit` matches and the total count.
pub fn search<'a>(
    root: &'a Node,
    text: Option<&str>,
    role: Option<&str>,
    capability: Option<&str>,
    limit: usize,
) -> (Vec<Match<'a>>, usize) {
    let query = text.map(str::to_lowercase);
    let mut matches = Vec::new();
    root.walk(&mut Vec::new(), &mut |node, path| {
        if role.is_some_and(|r| node.role != r) {
            return;
        }
        if capability.is_some_and(|c| {
            !node.capabilities().contains(&c) && !node.actions.iter().any(|a| a == c)
        }) {
            return;
        }
        let (reason, score) = match &query {
            Some(query) => match text_rank(node, query) {
                Some(rank) => rank,
                None => return,
            },
            None => ("filter", 1.0),
        };
        matches.push(Match {
            node,
            path,
            reason,
            score,
        });
    });
    matches.sort_by(|a, b| b.score.total_cmp(&a.score));
    let total = matches.len();
    matches.truncate(limit);
    (matches, total)
}

pub fn inspect(node: &Node) -> String {
    let mut lines = vec![node.line(), format!("role: {}", node.role)];
    let fields = [
        ("label", &node.label),
        ("value", &node.value),
        ("identifier", &node.identifier),
    ];
    for (name, field) in fields {
        if let Some(text) = field {
            lines.push(format!(
                "{name}: {}",
                serde_json::to_string(&clip(text, 400)).unwrap_or_default()
            ));
        }
    }
    if let Some(frame) = node.frame {
        lines.push(format!(
            "frame: x={} y={} w={} h={}",
            frame.x, frame.y, frame.w, frame.h
        ));
    }
    if !node.actions.is_empty() {
        lines.push(format!("actions: {}", node.actions.join(", ")));
    }
    lines.push(format!("children: {}", node.children.len()));
    lines.join("\n")
}
