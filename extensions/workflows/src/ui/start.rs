//! Starting a run from the UI: models marked `ask`, then the inputs.

use std::collections::BTreeMap;

use pi_ext_sdk::prelude::*;

use crate::model::{ASK, Workflow};
use crate::runs::Origin;
use crate::ui::fields::choose_model;
use crate::ui::{Screen, Ui};

/// The run id, or `None` when the user stops at a dialog or the start fails.
pub async fn run_dialog(
    ui: &Ui,
    screen: &mut Screen,
    mut workflow: Workflow,
) -> Result<Option<String>> {
    let name = workflow.name.clone();
    if workflow.agent.model.as_deref() == Some(ASK) {
        let title = format!("Default model for the steps of {name}");
        let Some(model) = choose_model(ui, screen, &title, false, "the session model").await?
        else {
            return Ok(None);
        };
        workflow.agent.model = model;
    }
    for step in &mut workflow.steps {
        if step.agent.model.as_deref() == Some(ASK) {
            let title = format!("Model for step {} of {name}", step.name);
            let Some(model) = choose_model(
                ui,
                screen,
                &title,
                false,
                "the workflow default or the session model",
            )
            .await?
            else {
                return Ok(None);
            };
            step.agent.model = model;
        }
    }
    let mut inputs = BTreeMap::new();
    for input in workflow.inputs() {
        let title = if input == "goal" {
            format!("Goal of {name}: what must the workflow reach?")
        } else {
            format!("{input} ({name})")
        };
        let Some(value) = ui.host.editor(&title, None).await? else {
            return Ok(None);
        };
        if value.trim().is_empty() {
            ui.host
                .notify_warning(format!("The run needs a value for {input}."))
                .await?;
            return Ok(None);
        }
        inputs.insert(input, value.trim().to_owned());
    }
    let background = workflow.background;
    match ui
        .app
        .start(&ui.host, workflow, inputs, Origin::User, background)
        .await
    {
        Ok(id) => {
            let mode = if background {
                "in the background"
            } else {
                "in the foreground (b in the run view moves it to the background)"
            };
            ui.host
                .notify_info(format!("Started {name} ({id}) {mode}."))
                .await?;
            Ok(Some(id))
        }
        Err(err) => {
            ui.host
                .notify_error(format!("Cannot start {name}: {err}"))
                .await?;
            Ok(None)
        }
    }
}
