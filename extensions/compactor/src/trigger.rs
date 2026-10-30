//! Percentage auto-compaction decisions with cooldown and repeat-growth guards.

use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AutoConfig {
    pub enabled: bool,
    pub threshold_percent: f64,
    pub cooldown_ms: u64,
    pub repeat_min_growth_tokens: u64,
}

impl AutoConfig {
    pub fn of(config: &crate::config::Config) -> Self {
        Self {
            enabled: config.trigger_percent,
            threshold_percent: config.threshold_percent.clamp(1.0, 99.0),
            cooldown_ms: config.cooldown_ms.min(24 * 60 * 60 * 1000),
            repeat_min_growth_tokens: config.repeat_min_growth_tokens.min(10_000_000),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Usage {
    pub tokens: u64,
    pub percent: f64,
}

impl Usage {
    /// Known usage from a `contextUsage` value; `None` when tokens or percent is missing.
    pub fn from_value(value: Option<&Value>) -> Option<Self> {
        let value = value?;
        let tokens = value
            .get("tokens")?
            .as_f64()
            .filter(|t| t.is_finite() && *t >= 0.0)?;
        let percent = value
            .get("percent")?
            .as_f64()
            .filter(|p| p.is_finite() && *p >= 0.0)?;
        Some(Self {
            tokens: tokens as u64,
            percent,
        })
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct AutoState {
    pub previous_percent: Option<f64>,
    pub previous_tokens: Option<u64>,
    pub in_flight: bool,
    pub last_trigger_at: Option<u64>,
    pub last_trigger_tokens: Option<u64>,
    pub repeat_baseline_tokens: Option<u64>,
    pub awaiting_post_compaction_sample: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    Disabled,
    UnknownUsage,
    InFlight,
    BelowThreshold,
    PostCompactionBaseline,
    ThresholdReached,
    ThresholdCrossed,
    CooldownActive,
    RepeatGrowthNeeded,
    RepeatGrowthReached,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Decision {
    pub trigger: bool,
    pub reason: Reason,
    pub state: AutoState,
    pub usage: Option<Usage>,
}

fn no(reason: Reason, state: AutoState, usage: Option<Usage>) -> Decision {
    Decision {
        trigger: false,
        reason,
        state,
        usage,
    }
}

fn fire(reason: Reason, usage: Usage, now: u64) -> Decision {
    let state = AutoState {
        previous_percent: Some(usage.percent),
        previous_tokens: Some(usage.tokens),
        in_flight: true,
        last_trigger_at: Some(now),
        last_trigger_tokens: Some(usage.tokens),
        repeat_baseline_tokens: Some(usage.tokens),
        awaiting_post_compaction_sample: false,
    };
    Decision {
        trigger: true,
        reason,
        state,
        usage: Some(usage),
    }
}

pub fn decide(config: AutoConfig, usage: Option<Usage>, state: &AutoState, now: u64) -> Decision {
    if !config.enabled {
        return no(Reason::Disabled, state.clone(), None);
    }
    if state.in_flight {
        return no(Reason::InFlight, state.clone(), None);
    }
    let Some(usage) = usage else {
        return no(Reason::UnknownUsage, state.clone(), None);
    };
    let threshold = config.threshold_percent;
    let mut next = AutoState {
        previous_percent: Some(usage.percent),
        previous_tokens: Some(usage.tokens),
        ..state.clone()
    };
    if state.awaiting_post_compaction_sample {
        let above = usage.percent >= threshold;
        next.awaiting_post_compaction_sample = false;
        next.repeat_baseline_tokens = above.then_some(usage.tokens);
        let reason = if above {
            Reason::PostCompactionBaseline
        } else {
            Reason::BelowThreshold
        };
        return no(reason, next, Some(usage));
    }
    if usage.percent < threshold {
        next.repeat_baseline_tokens = None;
        return no(Reason::BelowThreshold, next, Some(usage));
    }
    if let Some(last) = state.last_trigger_at
        && now.saturating_sub(last) < config.cooldown_ms
    {
        return no(Reason::CooldownActive, next, Some(usage));
    }
    match state.previous_percent {
        None => return fire(Reason::ThresholdReached, usage, now),
        Some(previous) if previous < threshold => {
            return fire(Reason::ThresholdCrossed, usage, now);
        }
        _ => {}
    }
    if state.last_trigger_at.is_none() {
        return fire(Reason::ThresholdReached, usage, now);
    }
    let baseline = state
        .repeat_baseline_tokens
        .or(state.last_trigger_tokens)
        .or(state.previous_tokens)
        .unwrap_or(usage.tokens);
    if usage.tokens.saturating_sub(baseline) < config.repeat_min_growth_tokens {
        return no(Reason::RepeatGrowthNeeded, next, Some(usage));
    }
    fire(Reason::RepeatGrowthReached, usage, now)
}

pub fn mark_complete(state: &AutoState) -> AutoState {
    AutoState {
        in_flight: false,
        awaiting_post_compaction_sample: true,
        ..state.clone()
    }
}

pub fn mark_error(state: &AutoState, now: u64) -> AutoState {
    AutoState {
        in_flight: false,
        awaiting_post_compaction_sample: false,
        last_trigger_at: state.last_trigger_at.or(Some(now)),
        ..state.clone()
    }
}
