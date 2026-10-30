use crate::client::DEFAULT_MAX_REQUESTS;
use crate::credentials::Env;

/// Daily caps from options and `PI_TYPESAFE_MAX_*`; the lower value wins.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Caps {
    pub max_requests_per_day: Option<u64>,
    pub max_input_tokens_per_day: Option<u64>,
    pub max_usd_per_day: Option<f64>,
}

impl Caps {
    pub fn from_env(env: &Env) -> Caps {
        let number = |name: &str| {
            env.get(name)
                .and_then(|raw| raw.parse::<f64>().ok())
                .filter(|v| v.is_finite() && *v > 0.0)
        };
        Caps {
            max_requests_per_day: number("PI_TYPESAFE_MAX_REQUESTS_PER_DAY")
                .map(|v| v.floor() as u64),
            max_input_tokens_per_day: number("PI_TYPESAFE_MAX_INPUT_TOKENS_PER_DAY")
                .map(|v| v.floor() as u64),
            max_usd_per_day: number("PI_TYPESAFE_MAX_USD_PER_DAY"),
        }
    }

    pub fn lowest(self, other: Caps) -> Caps {
        fn low<T: PartialOrd>(a: Option<T>, b: Option<T>) -> Option<T> {
            match (a, b) {
                (Some(a), Some(b)) => Some(if b < a { b } else { a }),
                (a, b) => a.or(b),
            }
        }
        Caps {
            max_requests_per_day: low(self.max_requests_per_day, other.max_requests_per_day),
            max_input_tokens_per_day: low(
                self.max_input_tokens_per_day,
                other.max_input_tokens_per_day,
            ),
            max_usd_per_day: low(self.max_usd_per_day, other.max_usd_per_day),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Blocked {
    pub cap: &'static str,
    pub limit: f64,
    pub used: f64,
    pub day: String,
}

pub fn cap_label(cap: &str) -> &'static str {
    match cap {
        "requestsPerDay" => "daily request cap",
        "inputTokensPerDay" => "daily input-token cap",
        _ => "daily spend cap",
    }
}

pub fn number(value: f64) -> String {
    format!("{}", (value * 1e6).round() / 1e6)
}

pub fn caps_description(caps: &Caps) -> String {
    let parts: Vec<String> = [
        Some(format!("{DEFAULT_MAX_REQUESTS} per session")),
        caps.max_requests_per_day
            .map(|v| format!("{v} requests per day")),
        caps.max_input_tokens_per_day
            .map(|v| format!("{v} input tokens per day")),
        caps.max_usd_per_day
            .map(|v| format!("${} per day", number(v))),
    ]
    .into_iter()
    .flatten()
    .collect();
    format!("Caps: {}.", parts.join(", "))
}
