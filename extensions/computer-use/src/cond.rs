use std::collections::BTreeSet;

use serde::Deserialize;

use crate::outline::Node;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Condition {
    #[serde(rename = "ref")]
    pub reference: Option<String>,
    pub scope_ref: Option<String>,
    pub text: Option<String>,
    pub role: Option<String>,
    pub value: Option<String>,
    pub until: Option<String>,
    pub timeout_ms: Option<f64>,
}

fn trimmed(value: &Option<String>) -> Option<&str> {
    value.as_deref().map(str::trim).filter(|v| !v.is_empty())
}

impl Condition {
    pub fn validate(&self) -> Result<(), String> {
        let (reference, scope) = (trimmed(&self.reference), trimmed(&self.scope_ref));
        let (text, role, value) = (
            trimmed(&self.text),
            trimmed(&self.role),
            trimmed(&self.value),
        );
        if reference.is_some() && scope.is_some() {
            return Err("A UI condition accepts ref or scopeRef, not both.".into());
        }
        if text.is_none() && role.is_none() && value.is_none() {
            return Err("A UI condition requires text, role, or value.".into());
        }
        if role.is_some()
            && text.is_none()
            && value.is_none()
            && reference.is_none()
            && scope.is_none()
        {
            return Err("A role-only UI condition requires ref or scopeRef.".into());
        }
        if value.is_some() && reference.is_none() {
            return Err("A value UI condition requires an exact ref.".into());
        }
        if let Some(until) = self
            .until
            .as_deref()
            .filter(|u| !matches!(*u, "present" | "absent"))
        {
            return Err(format!("until must be present or absent, not '{until}'."));
        }
        Ok(())
    }

    pub fn timeout_ms(&self) -> u64 {
        self.timeout_ms.unwrap_or(10_000.0).clamp(100.0, 60_000.0) as u64
    }

    pub fn wants_absent(&self) -> bool {
        self.until.as_deref() == Some("absent")
    }

    fn matches(&self, node: &Node) -> bool {
        if trimmed(&self.role).is_some_and(|role| node.role != role) {
            return false;
        }
        if trimmed(&self.value)
            .is_some_and(|value| node.value.as_deref().map(str::trim) != Some(value))
        {
            return false;
        }
        let Some(text) = trimmed(&self.text) else {
            return true;
        };
        let text = text.to_lowercase();
        let fields = [
            node.label.as_deref(),
            node.value.as_deref(),
            node.identifier.as_deref(),
        ];
        fields
            .into_iter()
            .flatten()
            .any(|field| field.to_lowercase().contains(&text))
    }

    /// Resolves refs to element paths in `base`, then tests the same paths in `current`.
    pub fn holds(&self, base: &Node, current: &Node) -> Result<bool, String> {
        let anchor = trimmed(&self.reference).or(trimmed(&self.scope_ref));
        let present = match anchor {
            None => any(current, &|node| self.matches(node)),
            Some(reference) => {
                let path = &base
                    .find(reference)
                    .ok_or_else(|| {
                        format!("Condition ref '{reference}' is unavailable in this state.")
                    })?
                    .path;
                match find_path(current, path) {
                    None => false,
                    Some(node) if self.reference.is_some() => self.matches(node),
                    Some(node) => any(node, &|n| self.matches(n)),
                }
            }
        };
        Ok(present != self.wants_absent())
    }
}

fn any(node: &Node, test: &dyn Fn(&Node) -> bool) -> bool {
    test(node) || node.children.iter().any(|child| any(child, test))
}

pub fn find_path<'a>(node: &'a Node, path: &[usize]) -> Option<&'a Node> {
    if node.path == path {
        return Some(node);
    }
    node.children
        .iter()
        .find_map(|child| find_path(child, path))
}

fn signature(node: &Node) -> String {
    let line = node.line();
    match line.split_once(' ') {
        Some((_, rest)) => rest.to_owned(),
        None => line,
    }
}

fn collect(node: &Node, out: &mut BTreeSet<String>) {
    out.insert(signature(node));
    for child in &node.children {
        collect(child, out);
    }
}

#[derive(Debug, Default, PartialEq)]
pub struct Changes {
    pub added: Vec<String>,
    pub removed: usize,
}

/// Compares node lines without refs; returns added lines and the removed count.
pub fn diff(before: &Node, after: &Node) -> Changes {
    let (mut old, mut new) = (BTreeSet::new(), BTreeSet::new());
    collect(before, &mut old);
    collect(after, &mut new);
    Changes {
        added: new.difference(&old).cloned().collect(),
        removed: old.difference(&new).count(),
    }
}

impl Changes {
    pub fn summary(&self, limit: usize) -> String {
        if self.added.is_empty() && self.removed == 0 {
            return "No outline changes.".into();
        }
        let mut lines = vec![format!(
            "Outline changes: {} added, {} removed.",
            self.added.len(),
            self.removed
        )];
        lines.extend(
            self.added
                .iter()
                .take(limit)
                .map(|line| format!("+ {line}")),
        );
        if self.added.len() > limit {
            lines.push(format!("+ … {} more", self.added.len() - limit));
        }
        lines.join("\n")
    }
}
