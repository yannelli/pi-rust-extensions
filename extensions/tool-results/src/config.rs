//! Settings of `<agentDir>/ext/tool-results.json` and per-tool rules.

use serde_json::{Map, Value, json};

pub const NAME: &str = "tool-results";
pub const FILE_NAME: &str = "tool-results.json";

/// What a JSON tool result becomes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JsonFormat {
    Toon,
    Compact,
    Stripped,
    Table,
    Off,
}

pub const JSON_FORMATS: [&str; 5] = ["toon", "compact", "stripped", "table", "off"];
pub const ERROR_MODES: [&str; 3] = ["clean", "all", "off"];

impl JsonFormat {
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "toon" => Self::Toon,
            "compact" => Self::Compact,
            "stripped" => Self::Stripped,
            "table" | "csv" => Self::Table,
            "off" => Self::Off,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        JSON_FORMATS[self as usize]
    }
}

/// How results with `isError` are treated: cleanup only, every feature, or untouched.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorMode {
    Clean,
    All,
    Off,
}

impl ErrorMode {
    pub fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "clean" => Self::Clean,
            "all" => Self::All,
            "off" => Self::Off,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        ERROR_MODES[self as usize]
    }
}

/// The settings for one tool call, after the global values and the matching rules.
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub enabled: bool,
    pub ansi: bool,
    pub progress: bool,
    pub control: bool,
    pub whitespace: bool,
    pub collapse_spaces: bool,
    pub dedupe: bool,
    pub dedupe_min: usize,
    pub base64: bool,
    pub base64_min: usize,
    pub stack_traces: bool,
    pub stack_max_frames: usize,
    pub long_lines: bool,
    pub long_line_chars: usize,
    pub json: JsonFormat,
    pub json_strip: bool,
    pub json_drop_defaults: bool,
    pub truncate: bool,
    pub threshold: usize,
    pub head_chars: usize,
    pub tail_chars: usize,
    pub keep_elided: bool,
    pub errors: ErrorMode,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: true,
            ansi: true,
            progress: true,
            control: true,
            whitespace: true,
            collapse_spaces: false,
            dedupe: true,
            dedupe_min: 3,
            base64: true,
            base64_min: 256,
            stack_traces: true,
            stack_max_frames: 30,
            long_lines: true,
            long_line_chars: 2000,
            json: JsonFormat::Toon,
            json_strip: false,
            json_drop_defaults: false,
            truncate: true,
            threshold: 20_000,
            head_chars: 4000,
            tail_chars: 2000,
            keep_elided: true,
            errors: ErrorMode::Clean,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Bool,
    /// A number with its minimum.
    Num(usize),
    Choice(&'static [&'static str]),
}

/// One editable key. `global` keys exist only at the top level of the file.
pub struct Field {
    pub key: &'static str,
    pub label: &'static str,
    pub kind: Kind,
    pub global: bool,
}

const fn field(key: &'static str, label: &'static str, kind: Kind) -> Field {
    Field {
        key,
        label,
        kind,
        global: false,
    }
}

const fn global(key: &'static str, label: &'static str, kind: Kind) -> Field {
    Field {
        key,
        label,
        kind,
        global: true,
    }
}

pub const FIELDS: &[Field] = &[
    field("enabled", "Enabled", Kind::Bool),
    global("dryRun", "Dry run (measure only)", Kind::Bool),
    global("defaultRules", "Built-in rule for read", Kind::Bool),
    field("errors", "Error results", Kind::Choice(&ERROR_MODES)),
    field("ansi", "Strip ANSI escapes", Kind::Bool),
    field("progress", "Progress bars and spinners", Kind::Bool),
    field("control", "Control characters", Kind::Bool),
    field("whitespace", "Trailing spaces, blank runs", Kind::Bool),
    field("collapseSpaces", "Collapse inner spaces", Kind::Bool),
    field("dedupe", "Collapse repeated lines", Kind::Bool),
    field("dedupeMin", "  Repeats before collapsing", Kind::Num(2)),
    field("base64", "Elide base64 and data URIs", Kind::Bool),
    field("base64Min", "  Minimum blob length", Kind::Num(16)),
    field("stackTraces", "Fold stack traces", Kind::Bool),
    field("stackMaxFrames", "  Frames kept per trace", Kind::Num(2)),
    field("longLines", "Clip long lines", Kind::Bool),
    field("longLineChars", "  Line length limit", Kind::Num(80)),
    field("json", "JSON format", Kind::Choice(&JSON_FORMATS)),
    field("jsonStrip", "  Drop null and empty fields", Kind::Bool),
    field("jsonDropDefaults", "  Also drop false and 0", Kind::Bool),
    field("truncate", "Store large results", Kind::Bool),
    field("threshold", "  Threshold (characters)", Kind::Num(500)),
    field("headChars", "  Preview head (characters)", Kind::Num(0)),
    field("tailChars", "  Preview tail (characters)", Kind::Num(0)),
    field(
        "keepElided",
        "  Store results with elided parts",
        Kind::Bool,
    ),
    global("contextChars", "Search context (characters)", Kind::Num(0)),
    global("maxMatches", "Search matches returned", Kind::Num(1)),
];

pub fn field_def(key: &str) -> Option<&'static Field> {
    FIELDS.iter().find(|f| f.key == key)
}

fn minimum(key: &str) -> usize {
    match field_def(key).map(|f| f.kind) {
        Some(Kind::Num(min)) => min,
        _ => 0,
    }
}

impl Settings {
    /// Applies the known keys of `map`; values of the wrong type are ignored.
    pub fn apply(&mut self, map: &Map<String, Value>) {
        for (key, value) in map {
            self.set(key, value);
        }
    }

    pub fn set(&mut self, key: &str, value: &Value) {
        let flag = value.as_bool();
        let num = value.as_u64().map(|n| (n as usize).max(minimum(key)));
        let text = value.as_str();
        macro_rules! put {
            ($slot:expr, $v:expr) => {
                if let Some(v) = $v {
                    $slot = v;
                }
            };
        }
        match key {
            "enabled" => put!(self.enabled, flag),
            "ansi" => put!(self.ansi, flag),
            "progress" => put!(self.progress, flag),
            "control" => put!(self.control, flag),
            "whitespace" => put!(self.whitespace, flag),
            "collapseSpaces" => put!(self.collapse_spaces, flag),
            "dedupe" => put!(self.dedupe, flag),
            "dedupeMin" => put!(self.dedupe_min, num),
            "base64" => put!(self.base64, flag),
            "base64Min" => put!(self.base64_min, num),
            "stackTraces" => put!(self.stack_traces, flag),
            "stackMaxFrames" => put!(self.stack_max_frames, num),
            "longLines" => put!(self.long_lines, flag),
            "longLineChars" => put!(self.long_line_chars, num),
            "json" => put!(self.json, text.and_then(JsonFormat::parse)),
            "jsonStrip" => put!(self.json_strip, flag),
            "jsonDropDefaults" => put!(self.json_drop_defaults, flag),
            "truncate" => put!(self.truncate, flag),
            "threshold" => put!(self.threshold, num),
            "headChars" => put!(self.head_chars, num),
            "tailChars" => put!(self.tail_chars, num),
            "keepElided" => put!(self.keep_elided, flag),
            "errors" => put!(self.errors, text.and_then(ErrorMode::parse)),
            _ => {}
        }
    }

    pub fn get(&self, key: &str) -> Value {
        match key {
            "enabled" => json!(self.enabled),
            "ansi" => json!(self.ansi),
            "progress" => json!(self.progress),
            "control" => json!(self.control),
            "whitespace" => json!(self.whitespace),
            "collapseSpaces" => json!(self.collapse_spaces),
            "dedupe" => json!(self.dedupe),
            "dedupeMin" => json!(self.dedupe_min),
            "base64" => json!(self.base64),
            "base64Min" => json!(self.base64_min),
            "stackTraces" => json!(self.stack_traces),
            "stackMaxFrames" => json!(self.stack_max_frames),
            "longLines" => json!(self.long_lines),
            "longLineChars" => json!(self.long_line_chars),
            "json" => json!(self.json.as_str()),
            "jsonStrip" => json!(self.json_strip),
            "jsonDropDefaults" => json!(self.json_drop_defaults),
            "truncate" => json!(self.truncate),
            "threshold" => json!(self.threshold),
            "headChars" => json!(self.head_chars),
            "tailChars" => json!(self.tail_chars),
            "keepElided" => json!(self.keep_elided),
            "errors" => json!(self.errors.as_str()),
            _ => Value::Null,
        }
    }

    /// Error results keep only the cleanup steps unless `errors` is `all`.
    pub fn for_error(&self) -> Option<Self> {
        match self.errors {
            ErrorMode::Off => None,
            ErrorMode::All => Some(self.clone()),
            ErrorMode::Clean => Some(Self {
                dedupe: false,
                base64: false,
                stack_traces: false,
                long_lines: false,
                json: JsonFormat::Off,
                truncate: false,
                ..self.clone()
            }),
        }
    }
}

/// A per-tool rule: a glob over tool names and the keys it overrides.
#[derive(Clone, Debug, PartialEq)]
pub struct Rule {
    pub pattern: String,
    pub overrides: Map<String, Value>,
    pub builtin: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    pub base: Settings,
    pub dry_run: bool,
    pub default_rules: bool,
    pub context_chars: usize,
    pub max_matches: usize,
    pub rules: Vec<Rule>,
}

impl Default for Config {
    fn default() -> Self {
        Self::from_value(&json!({}))
    }
}

/// `read` output is file text the model may edit, so only storage of large results applies.
pub fn builtin_rules() -> Vec<Rule> {
    let off = [
        "ansi",
        "progress",
        "control",
        "whitespace",
        "collapseSpaces",
        "dedupe",
        "base64",
        "stackTraces",
        "longLines",
    ];
    let mut overrides: Map<String, Value> =
        off.iter().map(|k| (k.to_string(), json!(false))).collect();
    overrides.insert("json".into(), json!("off"));
    vec![Rule {
        pattern: "read".into(),
        overrides,
        builtin: true,
    }]
}

impl Config {
    pub fn from_value(value: &Value) -> Self {
        let empty = Map::new();
        let root = value.as_object().unwrap_or(&empty);
        let mut base = Settings::default();
        base.apply(root);
        let num = |key: &str, default: usize| {
            root.get(key)
                .and_then(Value::as_u64)
                .map_or(default, |n| (n as usize).max(minimum(key)))
        };
        let flag =
            |key: &str, default: bool| root.get(key).and_then(Value::as_bool).unwrap_or(default);
        let default_rules = flag("defaultRules", true);
        let mut rules = if default_rules {
            builtin_rules()
        } else {
            Vec::new()
        };
        if let Some(tools) = root.get("tools").and_then(Value::as_object) {
            for (pattern, overrides) in tools {
                if let Some(overrides) = overrides.as_object() {
                    rules.push(Rule {
                        pattern: pattern.clone(),
                        overrides: overrides.clone(),
                        builtin: false,
                    });
                }
            }
        }
        Self {
            base,
            dry_run: flag("dryRun", false),
            default_rules,
            context_chars: num("contextChars", 200),
            max_matches: num("maxMatches", 20),
            rules,
        }
    }

    /// Global settings, then each matching rule in order; later rules win.
    pub fn for_tool(&self, tool: &str) -> Settings {
        let mut settings = self.base.clone();
        for rule in self.rules.iter().filter(|r| glob(&r.pattern, tool)) {
            settings.apply(&rule.overrides);
        }
        settings
    }

    /// The value of a top-level key, including the global-only ones.
    pub fn get(&self, key: &str) -> Value {
        match key {
            "dryRun" => json!(self.dry_run),
            "defaultRules" => json!(self.default_rules),
            "contextChars" => json!(self.context_chars),
            "maxMatches" => json!(self.max_matches),
            _ => self.base.get(key),
        }
    }
}

/// `*` matches any run of characters and `?` one character.
pub fn glob(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti, mut star, mut mark) = (0, 0, None, 0);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|c| *c == '*')
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
