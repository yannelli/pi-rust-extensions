//! `CHANGELOG.md` sections, the same layout as pi-rust.

use crate::version::{Change, Section, Version};

pub const HEADER: &str = "# Changelog\n\nAll notable changes to pi-rust-extensions are documented here. Versions follow [Semantic Versioning](https://semver.org/); before 1.0, breaking changes bump the minor version.\n";
const REPOSITORY: &str = "https://github.com/yannelli/pi-rust-extensions";

pub fn render_section(version: &Version, date: &str, changes: &[Change]) -> String {
    let mut text = format!("## [{version}] - {date}\n");
    for section in [
        Section::Breaking,
        Section::Added,
        Section::Changed,
        Section::Fixed,
    ] {
        let entries: Vec<String> = changes
            .iter()
            .filter(|change| change.section == section)
            .map(|change| {
                let scope = change
                    .scope
                    .as_deref()
                    .map(|scope| format!("**{scope}:** "))
                    .unwrap_or_default();
                let short = &change.hash[..change.hash.len().min(7)];
                format!(
                    "- {scope}{} ([{short}]({REPOSITORY}/commit/{}))",
                    change.description, change.hash
                )
            })
            .collect();
        if !entries.is_empty() {
            text.push_str(&format!(
                "\n### {}\n\n{}\n",
                section.title(),
                entries.join("\n")
            ));
        }
    }
    if changes.is_empty() {
        text.push_str("\nMaintenance release.\n");
    }
    text
}

pub fn insert_section(changelog: &str, section: &str) -> String {
    let changelog = if changelog.trim().is_empty() {
        HEADER.to_string()
    } else {
        changelog.to_string()
    };
    match changelog.find("\n## ") {
        Some(index) => format!(
            "{}\n{}\n{}",
            &changelog[..index],
            section.trim_end(),
            &changelog[index..]
        ),
        None => format!(
            "{}\n{}",
            changelog.trim_end_matches('\n').to_string() + "\n",
            section
        ),
    }
}

/// The body of `version`'s section, without its heading.
pub fn section_notes(changelog: &str, version: &Version) -> Option<String> {
    let heading = format!("## [{version}]");
    let start = changelog.find(&heading)?;
    let body_start = start + changelog[start..].find('\n')? + 1;
    let end = changelog[body_start..]
        .find("\n## ")
        .map(|offset| body_start + offset)
        .unwrap_or(changelog.len());
    Some(changelog[body_start..end].trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(section: Section, scope: Option<&str>, description: &str, hash: &str) -> Change {
        Change {
            section,
            scope: scope.map(String::from),
            description: description.into(),
            hash: hash.into(),
        }
    }

    #[test]
    fn sections_insert_and_extract() {
        let changes = vec![
            change(
                Section::Added,
                Some("deferred"),
                "tool search",
                "abcdef1234",
            ),
            change(Section::Fixed, None, "crash", "1234567890"),
        ];
        let first: Version = "0.5.0".parse().unwrap();
        let second: Version = "0.5.1".parse().unwrap();
        let changelog = insert_section("", &render_section(&first, "2026-11-02", &changes[..1]));
        let changelog =
            insert_section(&changelog, &render_section(&second, "2026-11-03", &changes));
        assert!(changelog.starts_with(HEADER));
        assert!(changelog.find("## [0.5.1]").unwrap() < changelog.find("## [0.5.0]").unwrap());
        let notes = section_notes(&changelog, &second).unwrap();
        assert!(
            notes.contains("### Added\n\n- **deferred:** tool search ([abcdef1]"),
            "{notes}"
        );
        assert!(notes.contains("### Fixed\n\n- crash"));
        assert!(!notes.contains("0.5.0"));
        assert!(
            section_notes(&changelog, &first)
                .unwrap()
                .contains("tool search")
        );
        assert!(render_section(&first, "d", &[]).contains("Maintenance release."));
    }
}
