//! Shared runtime state: the todo list, scopes, widget display state, and persistence.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, MutexGuard};

use pi_ext_sdk::{Host, Placement, Result, ToolResult};
use serde_json::Value;

use crate::config::Config;
use crate::format;
use crate::model::{TaskStatus, TodoState};
use crate::params::{self, Action};
use crate::reducer;
use crate::replay::{STATE_ENTRY_TYPE, replay};
use crate::scopes::{SCOPES_ENTRY_TYPE, ScopeEvent, ScopeStatus, Scopes, Visibility};
use crate::widget::{self, View, WIDGET_KEY};

#[derive(Default)]
pub struct Runtime {
    pub state: TodoState,
    pub scopes: Scopes,
    pub config: Config,
    pub collapsed: bool,
    pub pending_hide: HashSet<u64>,
    pub hidden: HashSet<u64>,
    pub pending_hide_scopes: HashSet<String>,
    pub hidden_scopes: HashSet<String>,
    widget_shown: bool,
}

impl Runtime {
    fn reset_display(&mut self) {
        self.pending_hide.clear();
        self.hidden.clear();
        self.pending_hide_scopes.clear();
        self.hidden_scopes.clear();
    }

    pub fn lines(&self, width: usize) -> Vec<String> {
        let view = View {
            state: &self.state,
            hidden: &self.hidden,
            collapsed: self.collapsed,
            scopes: &self.scopes,
            hidden_scopes: &self.hidden_scopes,
            config: &self.config,
        };
        widget::render(&view, width)
    }

    /// Completed tasks and finished scopes on screen now hide at the next `agent_start`.
    fn mark_displayed(&mut self) {
        let shown: Vec<u64> = widget::main_tasks(&self.state, &self.hidden)
            .iter()
            .filter(|t| t.status == TaskStatus::Completed)
            .map(|t| t.id)
            .collect();
        self.pending_hide.extend(shown);
        let finished = self
            .scopes
            .scopes
            .iter()
            .filter(|s| s.status != ScopeStatus::Running);
        let ids: Vec<String> = finished.map(|s| s.scope.clone()).collect();
        self.pending_hide_scopes.extend(ids);
    }
}

#[derive(Clone)]
pub struct Store {
    inner: Arc<Mutex<Runtime>>,
    scope: Option<String>,
}

impl Store {
    pub fn new(config: Config, scope: Option<String>) -> Self {
        let runtime = Runtime {
            config,
            ..Runtime::default()
        };
        Self {
            inner: Arc::new(Mutex::new(runtime)),
            scope,
        }
    }

    pub fn lock(&self) -> MutexGuard<'_, Runtime> {
        self.inner.lock().expect("todo state lock")
    }

    pub fn scope(&self) -> Option<&str> {
        self.scope.as_deref()
    }

    /// Runs one `todo` call: reduce, commit, persist when changed, refresh the widget.
    pub async fn execute(&self, host: &Host, args: Value) -> Result<ToolResult> {
        let args = params::prepare(args);
        let parsed =
            params::parse_action(&args).and_then(|action| Ok((action, params::parse(&args)?)));
        let (action, outcome) = {
            let mut runtime = self.lock();
            let current = runtime.state.clone();
            let (action, (state, op)) = match parsed {
                Ok((action, p)) => (action, reducer::apply(&current, action, &p)),
                Err(message) => (Action::List, reducer::error(&current, message)),
            };
            if state.next_id < current.next_id {
                runtime.reset_display();
            }
            runtime.state = state.clone();
            (action, (state, op))
        };
        let (state, op) = outcome;
        let details = format::details(action, args, &state, &op, self.scope());
        if op.changes_state() {
            host.append_entry(STATE_ENTRY_TYPE, &details).await?;
            self.refresh(host).await?;
        }
        Ok(ToolResult::text(format::content(&op, &state)).details(details))
    }

    /// Applies a TUI change through the reducer; returns the result text.
    pub async fn apply(&self, host: &Host, action: Action, args: Value) -> Result<String> {
        let result = self.execute(host, args_with(action, args)).await?;
        let text = result
            .content
            .first()
            .and_then(|c| c.get("text"))
            .and_then(Value::as_str);
        Ok(text.unwrap_or_default().to_owned())
    }

    pub async fn replay(&self, host: &Host) -> Result<()> {
        let entries = host.branch(None).await?;
        let (state, scopes) = replay(&entries);
        {
            let mut runtime = self.lock();
            runtime.state = state;
            runtime.scopes = scopes;
            runtime.reset_display();
        }
        self.refresh(host).await
    }

    pub async fn scope_event(&self, host: &Host, event: ScopeEvent) -> Result<()> {
        let changed = self.lock().scopes.apply(event);
        if changed {
            self.save_scopes(host).await?;
        }
        Ok(())
    }

    pub async fn set_visibility(
        &self,
        host: &Host,
        scope: &str,
        visibility: Visibility,
    ) -> Result<()> {
        let changed = self.lock().scopes.set_visibility(scope, visibility);
        if changed {
            self.save_scopes(host).await?;
        }
        Ok(())
    }

    async fn save_scopes(&self, host: &Host) -> Result<()> {
        let scopes = self.lock().scopes.clone();
        host.append_entry(SCOPES_ENTRY_TYPE, &scopes).await?;
        self.refresh(host).await
    }

    pub async fn agent_start(&self, host: &Host) -> Result<()> {
        {
            let mut runtime = self.lock();
            let pending: Vec<u64> = runtime.pending_hide.drain().collect();
            runtime.hidden.extend(pending);
            let scopes: Vec<String> = runtime.pending_hide_scopes.drain().collect();
            runtime.hidden_scopes.extend(scopes);
        }
        self.refresh(host).await
    }

    pub async fn toggle_collapse(&self, host: &Host) -> Result<()> {
        {
            let mut runtime = self.lock();
            runtime.collapsed = !runtime.collapsed;
        }
        self.refresh(host).await
    }

    /// Redraws the widget; children (`PI_TODO_SCOPE`) and hosts without UI draw nothing.
    pub async fn refresh(&self, host: &Host) -> Result<()> {
        if self.scope.is_some() || !host.has_ui() {
            return Ok(());
        }
        let width = host
            .surface_width(&format!("widget:{WIDGET_KEY}"))
            .unwrap_or(usize::MAX);
        let (lines, clear) = {
            let mut runtime = self.lock();
            let lines = runtime.lines(width);
            runtime.mark_displayed();
            let clear = lines.is_empty() && runtime.widget_shown;
            runtime.widget_shown = !lines.is_empty();
            (lines, clear)
        };
        if clear {
            host.clear_widget(WIDGET_KEY).await
        } else if !lines.is_empty() {
            host.set_widget(WIDGET_KEY, lines, Placement::AboveEditor)
                .await
        } else {
            Ok(())
        }
    }
}

fn args_with(action: Action, mut args: Value) -> Value {
    if let Some(obj) = args.as_object_mut() {
        obj.insert("action".into(), Value::String(action.as_str().into()));
    }
    args
}
