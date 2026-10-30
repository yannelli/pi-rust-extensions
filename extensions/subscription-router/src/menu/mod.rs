//! The `/router manage` menu.

pub mod accounts;
pub mod profiles;

use std::sync::Arc;

use pi_ext_sdk::prelude::*;

use crate::commands::{status_report, switch_slot};
use crate::state::State;

const ITEMS: [&str; 7] = [
    "Status",
    "Refresh usage",
    "Reset confirmed blocks",
    "Accounts",
    "Add account",
    "Switch slot",
    "Profiles",
];

/// Index of `choice` in `options`; `None` when the dialog was dismissed.
pub fn index_of<S: AsRef<str>>(options: &[S], choice: Option<String>) -> Option<usize> {
    let choice = choice?;
    options.iter().position(|o| o.as_ref() == choice)
}

pub async fn open(host: &Host, state: &Arc<State>) -> Result<()> {
    loop {
        let choice = host.select("Subscription router", &ITEMS).await?;
        match index_of(&ITEMS, choice) {
            None => return Ok(()),
            Some(0) => {
                host.notify_info(status_report(host, state, false).await)
                    .await?
            }
            Some(1) => {
                host.notify_info(status_report(host, state, true).await)
                    .await?
            }
            Some(2) => {
                state.router.tracker().clear();
                host.notify_info("subscription-router: cleared confirmed blocks")
                    .await?;
            }
            Some(3) => accounts::list(host, state).await?,
            Some(4) => accounts::add(host, state).await?,
            Some(5) => switch_slot(host, state, None).await?,
            Some(_) => profiles::list(host, state).await?,
        }
    }
}
