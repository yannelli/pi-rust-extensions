use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Instant;

use pi_ext_sdk::prelude::*;
use serde::{Deserialize, Serialize};

use crate::config::{self, Settings};
use crate::state::{Goal, GoalStatus};
use crate::store::Store;

pub const FOCUS_ENTRY: &str = "pi-goal-focus";
pub const DRAFT_ENTRY: &str = "pi-goal-draft";
pub const EVENT_MESSAGE: &str = "pi-goal-event";
pub const AUDIT_MESSAGE: &str = "pi-goal-audit-event";

pub const CORE_TOOLS: [&str; 3] = ["create_goal", "get_goal", "update_goal"];
pub const TASK_TOOLS: [&str; 2] = ["set_goal_tasks", "update_goal_task"];
pub const DRAFT_TOOLS: [&str; 3] = ["goal_question", "goal_questionnaire", "propose_goal_draft"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DraftMode {
    Goal,
    Sisyphus,
    Tweak,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Draft {
    pub mode: DraftMode,
    pub seed: String,
    #[serde(default)]
    pub target_goal_id: Option<String>,
    pub started_at: u64,
    #[serde(default)]
    pub cleared_at: Option<u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Focus {
    pub goal_id: Option<String>,
}

#[derive(Debug, Default)]
pub struct Run {
    pub started: Option<Instant>,
    pub stopped: bool,
    pub tool_calls: u32,
    pub idle_runs: u32,
    pub limit_notified: bool,
}

/// The detached auditor run of a goal; `generation` tells a stale run from the current one.
#[derive(Debug)]
pub struct AuditJob {
    pub generation: u64,
    pub abort: tokio::task::AbortHandle,
}

#[derive(Debug, Default)]
pub struct Inner {
    pub settings: Settings,
    pub store: Option<Store>,
    pub goals: Vec<Goal>,
    pub unreadable: usize,
    pub focus: Option<String>,
    pub draft: Option<Draft>,
    pub run: Run,
    pub expanded: bool,
    pub trust_notified: bool,
    pub audits: HashMap<String, AuditJob>,
    pub audit_generation: u64,
}

impl Inner {
    pub fn focused(&self) -> Option<&Goal> {
        let id = self.focus.as_deref()?;
        self.goals.iter().find(|goal| goal.id == id)
    }

    pub fn focused_mut(&mut self) -> Option<&mut Goal> {
        let id = self.focus.clone()?;
        self.goals.iter_mut().find(|goal| goal.id == id)
    }

    pub fn save(&self, goal: &Goal) {
        if let Some(store) = &self.store {
            if let Err(err) = store.save(goal) {
                pi_ext_sdk::warn!("cannot save goal {}: {err}", goal.id);
            }
        }
    }

    /// Saves the focused goal after `f` changes it.
    pub fn update_focused<R>(&mut self, f: impl FnOnce(&mut Goal) -> R) -> Option<R> {
        let goal = self.focused_mut()?;
        let result = f(goal);
        let goal = goal.clone();
        self.save(&goal);
        Some(result)
    }

    /// Moves a goal to the archive and drops it from the open list.
    pub fn archive(&mut self, id: &str) -> Option<Goal> {
        let index = self.goals.iter().position(|goal| goal.id == id)?;
        let goal = self.goals.remove(index);
        if let Some(store) = &self.store {
            if let Err(err) = store.archive(&goal) {
                pi_ext_sdk::warn!("cannot archive goal {}: {err}", goal.id);
            }
        }
        if self.focus.as_deref() == Some(id) {
            self.focus = None;
        }
        Some(goal)
    }

    pub fn open_goals(&self) -> impl Iterator<Item = &Goal> {
        self.goals.iter().filter(|goal| goal.status.is_open())
    }

    pub fn drafting(&self) -> bool {
        self.draft.is_some()
    }

    pub fn tasks_enabled(&self) -> bool {
        !self.settings.disable_tasks
    }

    /// Stops the running audit of the focused goal.
    pub fn cancel_focused_audit(&mut self) {
        if let Some(id) = self.focus.clone() {
            self.cancel_audit(&id);
        }
    }

    pub fn cancel_audit(&mut self, goal_id: &str) {
        if let Some(job) = self.audits.remove(goal_id) {
            job.abort.abort();
        }
    }
}

#[derive(Clone, Default)]
pub struct App {
    inner: Arc<Mutex<Inner>>,
    published: crate::active_work::Published,
}

impl App {
    pub fn published(&self) -> &crate::active_work::Published {
        &self.published
    }

    pub fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    /// Reads settings, goal files, and the focus and draft entries of the current branch.
    pub async fn reload(&self, host: &Host) -> Result<()> {
        let namespace = host.settings::<Value>("goal").await.unwrap_or(None);
        let trusted = project_trusted(host).await;
        let loaded = config::load(&host.agent_dir(), &host.cwd(), namespace.as_ref(), trusted);
        let store = Store::new(loaded.settings.goals_dir(&host.agent_dir(), &host.cwd()));
        let listing = store.list();
        let branch = host.branch(None).await.unwrap_or_default();
        let focus: Option<Focus> = custom_last(&branch, FOCUS_ENTRY);
        let draft: Option<Draft> = custom_last(&branch, DRAFT_ENTRY);
        let notify_trust = {
            let mut inner = self.lock();
            let goals = listing.goals;
            inner.focus = focus
                .and_then(|f| f.goal_id)
                .filter(|id| goals.iter().any(|g| &g.id == id && g.status.is_open()));
            inner.draft = draft.filter(|d| d.cleared_at.is_none());
            inner.goals = goals;
            inner.unreadable = listing.unreadable.len();
            inner.settings = loaded.settings;
            inner.store = Some(store);
            let Inner { goals, audits, .. } = &mut *inner;
            for goal in goals {
                if goal.status == GoalStatus::Auditing && !audits.contains_key(&goal.id) {
                    goal.status = GoalStatus::Active;
                }
            }
            let notify = !loaded.ignored.is_empty() && !inner.trust_notified;
            inner.trust_notified |= notify;
            notify.then(|| loaded.ignored.join(", "))
        };
        if let Some(keys) = notify_trust {
            let text = format!("goal: project is not trusted; ignored project settings {keys}");
            let _ = host.notify_warning(text).await;
        }
        Ok(())
    }

    /// Active tools minus every goal tool, plus the tools of the current profile.
    pub async fn install_profile(&self, host: &Host) -> Result<()> {
        let (drafting, tasks) = {
            let inner = self.lock();
            (inner.drafting(), inner.tasks_enabled())
        };
        let mut wanted: Vec<&str> = if drafting {
            DRAFT_TOOLS.to_vec()
        } else {
            CORE_TOOLS.to_vec()
        };
        if tasks && !drafting {
            wanted.extend(TASK_TOOLS);
        }
        let active = host.active_tools().await?;
        let all_goal: Vec<&str> = CORE_TOOLS
            .iter()
            .chain(&TASK_TOOLS)
            .chain(&DRAFT_TOOLS)
            .copied()
            .collect();
        let mut next: Vec<String> = active
            .iter()
            .filter(|name| !all_goal.contains(&name.as_str()))
            .cloned()
            .collect();
        next.extend(wanted.iter().map(|name| (*name).to_owned()));
        let mut sorted_active = active.clone();
        let mut sorted_next = next.clone();
        sorted_active.sort();
        sorted_next.sort();
        if sorted_active != sorted_next {
            host.set_active_tools(&next).await?;
        }
        Ok(())
    }

    pub async fn set_focus(&self, host: &Host, goal_id: Option<String>) -> Result<()> {
        self.lock().focus = goal_id.clone();
        host.append_entry(FOCUS_ENTRY, &Focus { goal_id }).await
    }

    pub async fn set_draft(&self, host: &Host, draft: Option<Draft>) -> Result<()> {
        let entry = {
            let mut inner = self.lock();
            let entry = match (&draft, &inner.draft) {
                (Some(draft), _) => Some(draft.clone()),
                (None, Some(old)) => Some(Draft {
                    cleared_at: Some(crate::state::now_ms()),
                    ..old.clone()
                }),
                (None, None) => None,
            };
            inner.draft = draft;
            entry
        };
        if let Some(entry) = entry {
            host.append_entry(DRAFT_ENTRY, &entry).await?;
        }
        Ok(())
    }
}

async fn project_trusted(host: &Host) -> bool {
    let from_snapshot = host
        .snapshot()
        .and_then(|s| s.extra.get("isProjectTrusted").and_then(Value::as_bool));
    if let Some(trusted) = from_snapshot {
        return trusted;
    }
    let answer = host.pi_call("ctx", "isProjectTrusted", vec![]).await;
    answer.ok().and_then(|v| v.as_bool()).unwrap_or(false)
}

fn custom_last<T: serde::de::DeserializeOwned>(branch: &[Value], custom_type: &str) -> Option<T> {
    branch
        .iter()
        .rev()
        .filter(|e| e.get("type").and_then(Value::as_str) == Some("custom"))
        .filter(|e| e.get("customType").and_then(Value::as_str) == Some(custom_type))
        .find_map(|e| e.get("data").and_then(|d| T::deserialize(d).ok()))
}
