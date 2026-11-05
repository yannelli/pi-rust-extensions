//! Files: workflow definitions in `<agentDir>/ext/workflows/definitions` and, in a trusted
//! project, `<cwd>/.pi/ext/workflows/definitions`; run directories in
//! `<agentDir>/ext/workflows/runs/<id>`; the bundled skills in `<agentDir>/ext/workflows/skills`.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::model::{Workflow, slug};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Global,
    Project,
}

impl Source {
    pub fn label(self) -> &'static str {
        match self {
            Source::Global => "global",
            Source::Project => "project",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Stored {
    pub workflow: Workflow,
    pub source: Source,
    pub path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct Dirs {
    pub global: PathBuf,
    /// `None` when the project is not trusted.
    pub project: Option<PathBuf>,
}

pub fn root(agent_dir: &Path) -> PathBuf {
    agent_dir.join("ext").join("workflows")
}

impl Dirs {
    pub fn new(agent_dir: &Path, project_dir: &Path, trusted: bool) -> Self {
        let project = pi_ext_sdk::paths::project_path(project_dir, "ext/workflows/definitions");
        Self {
            global: root(agent_dir).join("definitions"),
            project: trusted.then_some(project),
        }
    }

    pub fn dir(&self, source: Source) -> Option<&Path> {
        match source {
            Source::Global => Some(&self.global),
            Source::Project => self.project.as_deref(),
        }
    }

    /// All workflows by name; a project workflow hides a global one of the same name. The
    /// second list holds the files that do not parse.
    pub fn load(&self) -> (Vec<Stored>, Vec<String>) {
        let mut found: Vec<Stored> = Vec::new();
        let mut errors = Vec::new();
        for source in [Source::Project, Source::Global] {
            let Some(dir) = self.dir(source) else {
                continue;
            };
            let Ok(entries) = std::fs::read_dir(dir) else {
                continue;
            };
            let mut paths: Vec<PathBuf> = entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "json"))
                .collect();
            paths.sort();
            for path in paths {
                match read(&path) {
                    Ok(workflow) if found.iter().any(|s| s.workflow.name == workflow.name) => {}
                    Ok(workflow) => found.push(Stored {
                        workflow,
                        source,
                        path,
                    }),
                    Err(err) => errors.push(err),
                }
            }
        }
        found.sort_by(|a, b| a.workflow.name.cmp(&b.workflow.name));
        (found, errors)
    }

    pub fn find(&self, name: &str) -> Option<Stored> {
        let wanted = slug(name);
        self.load()
            .0
            .into_iter()
            .find(|s| s.workflow.name == wanted)
    }

    /// Writes `<slug>.json` to the source directory, after validation.
    pub fn save(&self, workflow: &Workflow, source: Source) -> Result<PathBuf, String> {
        let problems = workflow.problems();
        if !problems.is_empty() {
            return Err(problems.join("\n"));
        }
        let dir = self
            .dir(source)
            .ok_or("the project is not trusted, so workflows cannot be saved there")?;
        let mut workflow = workflow.clone();
        workflow.name = slug(&workflow.name);
        let path = dir.join(format!("{}.json", workflow.name));
        let text = serde_json::to_string_pretty(&workflow).map_err(|e| e.to_string())?;
        write_atomic(&path, &(text + "\n"))
            .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        Ok(path)
    }
}

/// A definition file; its file name is the workflow name.
pub fn read(path: &Path) -> Result<Workflow, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut workflow: Workflow =
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    workflow.name = path
        .file_stem()
        .map(|s| slug(&s.to_string_lossy()))
        .unwrap_or_default();
    Ok(workflow)
}

/// Writes a temporary file next to `path` and renames it, so readers never see a partial file.
/// The temporary name is unique, because a runner and pi sessions write at the same time.
pub fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let tmp = path.with_extension(format!("{}-{n}.tmp", std::process::id()));
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

pub fn runs_dir(agent_dir: &Path) -> PathBuf {
    root(agent_dir).join("runs")
}

const SKILLS: [(&str, &str); 2] = [
    (
        "workflow-create",
        include_str!("../skills/workflow-create.md"),
    ),
    (
        "workflow-optimize",
        include_str!("../skills/workflow-optimize.md"),
    ),
];

pub const FORMAT: &str = include_str!("../skills/format.md");

/// Writes the bundled skills (each with the file format appended) and returns their directory.
pub fn install_skills(agent_dir: &Path) -> std::io::Result<PathBuf> {
    let dir = root(agent_dir).join("skills");
    for (name, text) in SKILLS {
        let path = dir.join(name).join("SKILL.md");
        let text = format!("{text}\n{FORMAT}");
        if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
            write_atomic(&path, &text)?;
        }
    }
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn temp() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "wf-store-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    fn workflow(name: &str, description: &str) -> Workflow {
        serde_json::from_value(json!({
            "name": name, "description": description,
            "steps": [{ "name": "s", "tasks": [{ "prompt": "p" }] }]
        }))
        .unwrap()
    }

    #[test]
    fn project_hides_global_and_bad_files_are_reported() {
        let base = temp();
        let dirs = Dirs::new(&base.join("agent"), &base.join("work/.pi"), true);
        dirs.save(&workflow("Ship It", "global"), Source::Global)
            .unwrap();
        dirs.save(&workflow("ship-it", "project"), Source::Project)
            .unwrap();
        std::fs::write(dirs.global.join("broken.json"), "{").unwrap();
        let (found, errors) = dirs.load();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].workflow.name, "ship-it");
        assert_eq!(found[0].workflow.description, "project");
        assert_eq!(found[0].source, Source::Project);
        assert!(errors[0].contains("broken.json"), "{errors:?}");
        let untrusted = Dirs::new(&base.join("agent"), &base.join("work/.pi"), false);
        assert_eq!(
            untrusted.find("ship it").unwrap().workflow.description,
            "global"
        );
        assert!(untrusted.save(&workflow("x", ""), Source::Project).is_err());
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn save_rejects_invalid_workflows() {
        let base = temp();
        let dirs = Dirs::new(&base, &base, false);
        let error = dirs.save(&Workflow::default(), Source::Global).unwrap_err();
        assert!(error.contains("needs a name"), "{error}");
        let skills = install_skills(&base).unwrap();
        let text = std::fs::read_to_string(skills.join("workflow-create/SKILL.md")).unwrap();
        assert!(text.starts_with("---\nname: workflow-create"), "{text}");
        let _ = std::fs::remove_dir_all(base);
    }
}
