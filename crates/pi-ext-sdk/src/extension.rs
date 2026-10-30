use std::future::Future;
use std::process::ExitCode;

use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncWrite};

use crate::ctx::{CommandCtx, EventCtx, ToolCtx};
use crate::error::Result;
use crate::handler;
use crate::host::Host;
use crate::registry::{Command, Registry, VirtualModel};
use crate::reply::{EventReply, ToolResult};
use crate::spec::{
    AutocompleteRequest, CommandSpec, CompletionItem, FlagSpec, RouteRequest, ShortcutSpec,
    Suggestions, ToolSpec,
};
use crate::surface::SurfaceEvent;

/// Builder for a native extension. Finish with [`Extension::run`].
pub struct Extension {
    pub(crate) name: String,
    pub(crate) registry: Registry,
}

impl Extension {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            registry: Registry::default(),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Adds a tool. `A` is the argument type: a serde struct, or `serde_json::Value`.
    pub fn tool<A, R, F, Fut>(mut self, spec: ToolSpec, run: F) -> Self
    where
        A: DeserializeOwned,
        R: Into<ToolResult>,
        F: Fn(A, ToolCtx) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<R>> + Send + 'static,
    {
        self.registry.put_tool(spec, handler::tool(run));
        self
    }

    /// Adds a slash command; `run` gets the text after the command name.
    pub fn command<F, Fut>(mut self, name: &str, description: &str, run: F) -> Self
    where
        F: Fn(String, CommandCtx) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        let spec = CommandSpec {
            name: name.to_owned(),
            description: Some(description.to_owned()).filter(|d| !d.is_empty()),
            completions: false,
        };
        let run = handler::command(run);
        self.registry.put_command(Command {
            spec,
            run,
            complete: None,
        });
        self
    }

    /// Adds argument completions to a command added before with [`Extension::command`].
    /// Panics when no command `name` exists.
    pub fn completions<F, Fut>(mut self, name: &str, complete: F) -> Self
    where
        F: Fn(String, Host) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Option<Vec<CompletionItem>>>> + Send + 'static,
    {
        let command = self
            .registry
            .commands
            .iter_mut()
            .find(|c| c.spec.name == name);
        let command = command.unwrap_or_else(|| panic!("completions for unknown command {name}"));
        command.spec.completions = true;
        command.complete = Some(handler::with_host(complete));
        self
    }

    /// Adds a keyboard shortcut such as `ctrl+shift+t`.
    pub fn shortcut<F, Fut>(mut self, key: &str, description: &str, run: F) -> Self
    where
        F: Fn(CommandCtx) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        let spec = ShortcutSpec {
            shortcut: key.to_owned(),
            description: Some(description.to_owned()).filter(|d| !d.is_empty()),
        };
        self.registry.shortcuts.push((spec, handler::shortcut(run)));
        self
    }

    pub fn flag(mut self, flag: FlagSpec) -> Self {
        self.registry.flags.push(flag);
        self
    }

    /// Handles a pi event. Handlers of one event run in order; each sees earlier patches,
    /// and a later non-null result replaces an earlier one.
    pub fn on<R, F, Fut>(mut self, event: &str, run: F) -> Self
    where
        R: Into<EventReply>,
        F: Fn(EventCtx) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<R>> + Send + 'static,
    {
        let run = handler::event(run);
        self.registry.events.push((event.to_owned(), run));
        self
    }

    /// Declares a provider (`baseUrl`, `api`, `apiKey`, `headers`, `models`, `oauth`).
    pub fn provider(mut self, name: &str, config: Value) -> Self {
        self.registry.providers.push((name.to_owned(), config));
        self
    }

    /// Declares a virtual model; `definition` needs `provider` and `id`.
    pub fn virtual_model<F, Fut>(mut self, definition: Value, route: F) -> Self
    where
        F: Fn(RouteRequest, Host) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Value>> + Send + 'static,
    {
        let route = handler::with_host(route);
        let model = VirtualModel { definition, route };
        self.registry.virtual_models.push(model);
        self
    }

    /// Editor autocomplete for `triggers` (for example `["@"]`). `None` keeps pi's suggestions.
    pub fn autocomplete<F, Fut>(mut self, triggers: &[&str], suggest: F) -> Self
    where
        F: Fn(AutocompleteRequest, Host) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Option<Suggestions>>> + Send + 'static,
    {
        let triggers = triggers.iter().map(|t| (*t).to_owned()).collect();
        let suggest = handler::with_host(suggest);
        self.registry.autocomplete.push((triggers, suggest));
        self
    }

    /// Runs in its own task at the first `session_start`, when the host snapshot holds the session ctx.
    pub fn on_start<F, Fut>(mut self, start: F) -> Self
    where
        F: Fn(Host) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        self.registry.on_start.push(handler::host_only(start));
        self
    }

    /// Runs once on `shutdown`, `exit`, or stdin EOF, with a 1.5 second limit.
    pub fn on_shutdown<F, Fut>(mut self, stop: F) -> Self
    where
        F: Fn(Host) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        self.registry.on_shutdown.push(handler::host_only(stop));
        self
    }

    /// Runs for each resize or close of a widget or footer surface.
    pub fn on_surface<F, Fut>(mut self, handle: F) -> Self
    where
        F: Fn(SurfaceEvent, Host) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        self.registry.on_surface.push(handler::with_host(handle));
        self
    }

    /// Runs with the new footer data (`gitBranch`, `sessionStats`, `runState`) each time pi
    /// sends `footer.changed`; pi sends it in `tui` mode only.
    pub fn on_footer_change<F, Fut>(mut self, handle: F) -> Self
    where
        F: Fn(Value, Host) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<()>> + Send + 'static,
    {
        self.registry.on_footer.push(handler::with_host(handle));
        self
    }

    /// Serves the protocol on stdin and stdout. Nonzero exit on a protocol failure.
    pub fn run(self) -> ExitCode {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build();
        let runtime = match runtime {
            Ok(runtime) => runtime,
            Err(err) => {
                crate::error!("cannot start the tokio runtime: {err}");
                return ExitCode::FAILURE;
            }
        };
        let result = runtime.block_on(self.serve(tokio::io::stdin(), tokio::io::stdout()));
        runtime.shutdown_background();
        match result {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                crate::error!("protocol failure: {err}");
                ExitCode::FAILURE
            }
        }
    }

    /// Serves the protocol on any reader and writer until `exit` or EOF.
    pub async fn serve<R, W>(self, reader: R, writer: W) -> Result<()>
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        crate::server::serve(self, reader, writer).await
    }
}
