//! Lines of the `/claude-plugins` panel.

use pi_ext_sdk::style::{self, Color};

use super::{Entry, Inventory, Market, Plugin, Screen, Special, TABS, Tab, Tone, View, scope_word};
use crate::env::Scope;
use crate::listing::Status;

/// Lines around the list: rule, title, tabs, search, blanks, hints, rule.
const CHROME: usize = 8;
const MAX_VISIBLE: usize = 12;
pub const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

fn accent(text: &str) -> String {
    style::fg(Color::Cyan, text)
}

fn muted(text: &str) -> String {
    style::fg(Color::Gray, text)
}

fn warning(text: &str) -> String {
    style::fg(Color::Yellow, text)
}

fn good(text: &str) -> String {
    style::fg(Color::Green, text)
}

fn bad(text: &str) -> String {
    style::fg(Color::Red, text)
}

/// Splits `text` into lines of at most `width` columns, at spaces where it can.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(10);
    let mut out = Vec::new();
    for paragraph in text.lines() {
        let mut line = String::new();
        for word in paragraph.split(' ') {
            let fits = style::visible_width(&line) + 1 + style::visible_width(word) <= width;
            if !line.is_empty() && !fits {
                out.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        out.push(line);
    }
    out
}

fn tab_label(view: &View, tab: Tab, inv: &Inventory) -> String {
    let count = view.count(inv, tab);
    match tab {
        Tab::Installed => format!("Installed ({count})"),
        Tab::Discover => format!("Discover ({count})"),
        Tab::Marketplaces => format!("Marketplaces ({count})"),
    }
}

fn scope_tag(scope: Scope) -> String {
    match scope {
        Scope::User => muted("user"),
        Scope::Project => accent("project"),
    }
}

fn plugin_glyph(p: &Plugin) -> String {
    match (p.row.enabled, p.row.status) {
        (Some(true), _) => good("●"),
        (Some(false), _) => muted("○"),
        (None, Status::Unavailable) => bad("✕"),
        (None, _) => accent("+"),
    }
}

fn contents(p: &Plugin) -> String {
    let mut parts = Vec::new();
    let mut add = |n: usize, one: &str, many: &str| {
        if n > 0 {
            parts.push(format!("{n} {}", if n == 1 { one } else { many }));
        }
    };
    add(p.skills, "skill", "skills");
    add(p.commands, "command", "commands");
    add(p.agents, "agent", "agents");
    add(p.mcp.len(), "MCP server", "MCP servers");
    if p.hooks {
        parts.push("hooks".into());
    }
    parts.join(", ")
}

fn plugin_line(p: &Plugin, name_width: usize) -> String {
    let name = format!("{:<name_width$}", p.id());
    let version = p
        .row
        .version
        .as_deref()
        .map_or(String::new(), |v| format!(" {}", muted(&format!("v{v}"))));
    let state = match (p.row.enabled, p.row.status) {
        (Some(true), _) => format!(" {}", scope_tag(p.row.scope)),
        (Some(false), _) => format!(" {} {}", muted("off"), scope_tag(p.row.scope)),
        (None, Status::Unavailable) => format!(" {}", bad("unavailable")),
        (None, _) => String::new(),
    };
    let detail = match p.row.description.as_deref() {
        Some(text) if !text.is_empty() => format!("  {}", muted(text)),
        _ => String::new(),
    };
    format!("{} {name}{version}{state}{detail}", plugin_glyph(p))
}

fn market_line(m: &Market, name_width: usize) -> String {
    let glyph = if m.auto_update {
        good("⟳")
    } else {
        muted("·")
    };
    let name = format!("{:<name_width$}", m.name);
    let installed = muted(&format!("{}/{} installed", m.installed, m.plugins));
    let auto = if m.auto_update {
        format!(" {}", good("auto-update"))
    } else {
        String::new()
    };
    format!(
        "{glyph} {name} {} {installed}{auto}  {}",
        scope_tag(m.scope),
        muted(&m.source)
    )
}

fn special_line(special: Special) -> String {
    match special {
        Special::Add => accent("+ Add a marketplace…"),
        Special::UpdateAll => accent("⟳ Update every marketplace and plugin"),
        Special::Import => accent("⇣ Import from Claude Code settings"),
    }
}

impl View {
    pub fn render(&self, inv: &Inventory, width: usize, height: Option<usize>) -> Vec<String> {
        let width = width.max(24);
        let rule = muted(&"─".repeat(width));
        let mut lines = vec![rule.clone()];
        let summary = muted(&format!(
            "  {} installed · {} marketplace{} · changes apply after /reload",
            inv.installed(),
            inv.markets.len(),
            if inv.markets.len() == 1 { "" } else { "s" }
        ));
        lines.push(format!(
            "{}{summary}",
            accent(&style::bold(" Claude plugins"))
        ));
        let tabs: Vec<String> = TABS
            .iter()
            .enumerate()
            .map(|(i, tab)| match i == self.tab {
                true => accent(&style::bold(&format!("[{}]", tab_label(self, *tab, inv)))),
                false => muted(&tab_label(self, *tab, inv)),
            })
            .collect();
        lines.push(format!(" {}", tabs.join("  ")));
        lines.push(match self.query.is_empty() {
            true => muted(" › type to search"),
            false => format!(" › {}▏", self.query),
        });
        lines.push(String::new());
        let status = self.status_lines(width);
        let budget = height.map_or(MAX_VISIBLE, |h| {
            h.saturating_sub(CHROME + status.len()).clamp(1, 30)
        });
        if let Some(error) = &inv.error {
            lines.extend(
                wrap(&format!("Cannot read the marketplaces: {error}"), width - 2)
                    .iter()
                    .map(|l| warning(&format!(" {l}"))),
            );
        }
        match &self.screen {
            Screen::List => lines.extend(self.list(inv, budget)),
            Screen::Menu { choice } => lines.extend(self.menu(inv, *choice, width)),
            Screen::Confirm { question, .. } => {
                lines.extend(
                    wrap(question, width - 2)
                        .iter()
                        .map(|l| warning(&format!(" {l}"))),
                );
            }
            Screen::Add { source, scope } => {
                lines.push(accent(&style::bold(" Add a marketplace")));
                lines.push(muted(" GitHub owner/repo, a git URL (https, ssh, git@), a local folder, or claude-plugins-official"));
                lines.push(format!(" › {source}▏"));
                let scopes: Vec<String> = self
                    .scopes()
                    .into_iter()
                    .map(|s| {
                        let label = format!("for {}", scope_word(s));
                        if s == *scope {
                            accent(&format!("[{label}]"))
                        } else {
                            muted(&label)
                        }
                    })
                    .collect();
                lines.push(format!(" {}", scopes.join("  ")));
            }
        }
        lines.push(String::new());
        lines.extend(status);
        lines.push(muted(&self.hints()));
        lines.push(rule);
        if let Some(height) = height {
            lines.truncate(height.max(1));
        }
        lines
            .into_iter()
            .map(|line| style::truncate(&line, width))
            .collect()
    }

    fn status_lines(&self, width: usize) -> Vec<String> {
        let Some((tone, text)) = &self.status else {
            return Vec::new();
        };
        let mut wrapped = wrap(text, width.saturating_sub(4));
        // Long results (several plugins updated) keep their first lines; the rest is in the log.
        if wrapped.len() > 6 {
            let more = wrapped.len() - 5;
            wrapped.truncate(5);
            wrapped.push(format!("… {more} more lines"));
        }
        wrapped
            .into_iter()
            .enumerate()
            .map(|(i, line)| match tone {
                Tone::Busy if i == 0 => {
                    accent(&format!(" {} {line}", SPINNER[self.frame % SPINNER.len()]))
                }
                Tone::Busy => accent(&format!("   {line}")),
                Tone::Warn => warning(&format!(" {line}")),
                Tone::Info => muted(&format!(" {line}")),
            })
            .collect()
    }

    fn list(&self, inv: &Inventory, budget: usize) -> Vec<String> {
        let entries = self.entries(inv);
        if entries.is_empty() {
            let text = match (self.query.is_empty(), self.tab()) {
                (false, _) => " No matches".to_owned(),
                (true, Tab::Installed) => {
                    " No plugins installed. Find some under Discover (tab).".to_owned()
                }
                (true, Tab::Discover) => {
                    " Nothing to install. Add a marketplace under Marketplaces (tab).".to_owned()
                }
                (true, Tab::Marketplaces) => " No marketplaces.".to_owned(),
            };
            return vec![muted(&text)];
        }
        let visible = budget.min(entries.len());
        let selected = self.selected.min(entries.len() - 1);
        let start = selected
            .saturating_sub(visible / 2)
            .min(entries.len() - visible);
        let name_width = entries[start..start + visible]
            .iter()
            .map(|entry| match entry {
                Entry::Plugin(p) => style::visible_width(&p.id()),
                Entry::Market(m) => style::visible_width(&m.name),
                Entry::Special(_) => 0,
            })
            .max()
            .unwrap_or(0)
            .min(40);
        let mut lines: Vec<String> = entries[start..start + visible]
            .iter()
            .enumerate()
            .map(|(i, entry)| {
                let pointer = if start + i == selected {
                    accent("→ ")
                } else {
                    "  ".to_owned()
                };
                let text = match entry {
                    Entry::Plugin(p) => plugin_line(p, name_width),
                    Entry::Market(m) => market_line(m, name_width),
                    Entry::Special(s) => special_line(*s),
                };
                format!("{pointer}{text}")
            })
            .collect();
        if entries.len() > visible {
            lines.push(muted(&format!("  ({}/{})", selected + 1, entries.len())));
        }
        lines
    }

    fn menu(&self, inv: &Inventory, choice: usize, width: usize) -> Vec<String> {
        let mut lines = Vec::new();
        let text_width = width.saturating_sub(4);
        let mut field = |label: &str, value: String| {
            for (i, line) in wrap(&value, text_width.saturating_sub(14))
                .into_iter()
                .enumerate()
            {
                let head = if i == 0 {
                    format!("{label:<13}")
                } else {
                    " ".repeat(13)
                };
                lines.push(format!(" {} {line}", muted(&head)));
            }
        };
        match self.entries(inv).get(self.selected) {
            Some(Entry::Plugin(p)) => {
                field("plugin", p.id());
                if let Some(version) = &p.row.version {
                    field("version", version.clone());
                }
                let state = match p.row.enabled {
                    Some(true) => format!("on, for {}", scope_word(p.row.scope)),
                    Some(false) => format!("off, installed for {}", scope_word(p.row.scope)),
                    None if p.row.status == Status::Unavailable => "cannot be installed".into(),
                    None => "not installed".into(),
                };
                field("state", state);
                if let Some(description) = &p.row.description {
                    field("about", description.clone());
                }
                let added = contents(p);
                if !added.is_empty() {
                    field("adds", added);
                }
                if !p.row.reasons.is_empty() {
                    field("notes", p.row.reasons.join("; "));
                }
            }
            Some(Entry::Market(m)) => {
                field("marketplace", m.name.clone());
                field("source", m.source.clone());
                field("for", scope_word(m.scope).into());
                field(
                    "plugins",
                    format!("{} installed of {}", m.installed, m.plugins),
                );
                field(
                    "auto-update",
                    if m.auto_update {
                        "on, at session start".into()
                    } else {
                        "off".into()
                    },
                );
            }
            _ => {}
        }
        lines.push(String::new());
        lines.push(accent(&style::bold(" Actions")));
        let choices = self.choices(inv);
        if choices.is_empty() {
            lines.push(muted("   Nothing to do here."));
        }
        for (i, item) in choices.iter().enumerate() {
            let label = if item.confirm {
                warning(&item.label)
            } else {
                item.label.clone()
            };
            lines.push(if i == choice {
                format!("{}{}", accent(" → "), accent(&style::bold(&item.label)))
            } else {
                format!("   {label}")
            });
        }
        lines
    }
}
