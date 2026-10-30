use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::ctx::{CommandCtx, EventCtx, ToolCtx};
use crate::error::{Error, Result};
use crate::host::Host;
use crate::reply::{EventReply, ToolResult};
use crate::spec::{AutocompleteRequest, CompletionItem, RouteRequest, Suggestions};
use crate::surface::SurfaceEvent;

pub(crate) type BoxFuture<T> = Pin<Box<dyn Future<Output = Result<T>> + Send>>;
pub(crate) type ToolFn = Arc<dyn Fn(Value, ToolCtx) -> BoxFuture<ToolResult> + Send + Sync>;
pub(crate) type EventFn = Arc<dyn Fn(EventCtx) -> BoxFuture<EventReply> + Send + Sync>;
pub(crate) type CommandFn = Arc<dyn Fn(String, CommandCtx) -> BoxFuture<()> + Send + Sync>;
pub(crate) type ShortcutFn = Arc<dyn Fn(CommandCtx) -> BoxFuture<()> + Send + Sync>;
pub(crate) type CompleteFn =
    Arc<dyn Fn(String, Host) -> BoxFuture<Option<Vec<CompletionItem>>> + Send + Sync>;
pub(crate) type AutocompleteFn =
    Arc<dyn Fn(AutocompleteRequest, Host) -> BoxFuture<Option<Suggestions>> + Send + Sync>;
pub(crate) type RouteFn = Arc<dyn Fn(RouteRequest, Host) -> BoxFuture<Value> + Send + Sync>;
pub(crate) type HostFn = Arc<dyn Fn(Host) -> BoxFuture<()> + Send + Sync>;
pub(crate) type SurfaceFn = Arc<dyn Fn(SurfaceEvent, Host) -> BoxFuture<()> + Send + Sync>;

pub(crate) fn tool<A, R, F, Fut>(f: F) -> ToolFn
where
    A: DeserializeOwned,
    R: Into<ToolResult>,
    F: Fn(A, ToolCtx) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<R>> + Send + 'static,
{
    Arc::new(move |args, ctx| match serde_json::from_value::<A>(args) {
        Ok(args) => {
            let fut = f(args, ctx);
            Box::pin(async move { fut.await.map(Into::into) })
        }
        Err(err) => Box::pin(std::future::ready(Err(Error::invalid_params(err)))),
    })
}

pub(crate) fn event<R, F, Fut>(f: F) -> EventFn
where
    R: Into<EventReply>,
    F: Fn(EventCtx) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<R>> + Send + 'static,
{
    Arc::new(move |ctx| {
        let fut = f(ctx);
        Box::pin(async move { fut.await.map(Into::into) })
    })
}

pub(crate) fn command<F, Fut>(f: F) -> CommandFn
where
    F: Fn(String, CommandCtx) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<()>> + Send + 'static,
{
    Arc::new(move |args, ctx| Box::pin(f(args, ctx)))
}

pub(crate) fn shortcut<F, Fut>(f: F) -> ShortcutFn
where
    F: Fn(CommandCtx) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<()>> + Send + 'static,
{
    Arc::new(move |ctx| Box::pin(f(ctx)))
}

pub(crate) fn with_host<A, T, F, Fut>(f: F) -> Arc<dyn Fn(A, Host) -> BoxFuture<T> + Send + Sync>
where
    A: 'static,
    F: Fn(A, Host) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<T>> + Send + 'static,
{
    Arc::new(move |arg, host| Box::pin(f(arg, host)))
}

pub(crate) fn host_only<F, Fut>(f: F) -> HostFn
where
    F: Fn(Host) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<()>> + Send + 'static,
{
    Arc::new(move |host| Box::pin(f(host)))
}
