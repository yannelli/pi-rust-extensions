use std::path::{Path, PathBuf};

use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagedBrowser {
    Chrome,
    Helium,
}

impl ManagedBrowser {
    fn parse(value: &Value) -> Option<Self> {
        match value.as_str()? {
            "chrome" => Some(Self::Chrome),
            "helium" => Some(Self::Helium),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Chrome => "chrome",
            Self::Helium => "helium",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    pub browser_use: bool,
    pub headless: bool,
    pub cursor_overlay: bool,
    pub managed_browser: ManagedBrowser,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            browser_use: true,
            headless: false,
            cursor_overlay: true,
            managed_browser: ManagedBrowser::Chrome,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Partial {
    pub browser_use: Option<bool>,
    pub headless: Option<bool>,
    pub cursor_overlay: Option<bool>,
    pub managed_browser: Option<ManagedBrowser>,
}

impl Partial {
    pub fn keys(&self) -> Vec<&'static str> {
        let mut keys = Vec::new();
        if self.browser_use.is_some() {
            keys.push("browser_use");
        }
        if self.headless.is_some() {
            keys.push("headless");
        }
        if self.cursor_overlay.is_some() {
            keys.push("cursor_overlay");
        }
        if self.managed_browser.is_some() {
            keys.push("managed_browser");
        }
        keys
    }
}

impl Config {
    pub fn apply(&mut self, partial: &Partial) {
        self.browser_use = partial.browser_use.unwrap_or(self.browser_use);
        self.headless = partial.headless.unwrap_or(self.headless);
        self.cursor_overlay = partial.cursor_overlay.unwrap_or(self.cursor_overlay);
        self.managed_browser = partial.managed_browser.unwrap_or(self.managed_browser);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceStatus {
    Loaded,
    NotFound,
    Untrusted,
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub path: PathBuf,
    pub status: SourceStatus,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Loaded {
    pub config: Config,
    pub sources: Vec<Source>,
    pub env: Partial,
}

pub fn parse_bool(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(flag) => Some(*flag),
        Value::Number(number) => match number.as_f64() {
            Some(1.0) => Some(true),
            Some(0.0) => Some(false),
            _ => None,
        },
        Value::String(text) => match text.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "on" | "enabled" => Some(true),
            "0" | "false" | "no" | "off" | "disabled" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

pub fn normalize(raw: &Value) -> Partial {
    let source = match raw.get("computer_use") {
        Some(nested) if nested.is_object() => nested,
        _ => raw,
    };
    let get = |key: &str| source.get(key).unwrap_or(&Value::Null);
    Partial {
        browser_use: parse_bool(get("browser_use")),
        headless: parse_bool(get("headless")),
        cursor_overlay: parse_bool(get("cursor_overlay")),
        managed_browser: ManagedBrowser::parse(get("managed_browser")),
    }
}

pub fn from_env(lookup: impl Fn(&str) -> Option<String>) -> Partial {
    let value = |key: &str| lookup(key).map(Value::String).unwrap_or(Value::Null);
    Partial {
        browser_use: parse_bool(&value("PI_COMPUTER_USE_BROWSER_USE")),
        headless: parse_bool(&value("PI_COMPUTER_USE_HEADLESS")),
        cursor_overlay: parse_bool(&value("PI_COMPUTER_USE_CURSOR_OVERLAY")),
        managed_browser: ManagedBrowser::parse(&value("PI_COMPUTER_USE_MANAGED_BROWSER")),
    }
}

fn read(path: &Path) -> (Source, Partial) {
    let source = |status| Source {
        path: path.to_path_buf(),
        status,
    };
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return (source(SourceStatus::NotFound), Partial::default());
        }
        Err(err) => {
            return (
                source(SourceStatus::Error(err.to_string())),
                Partial::default(),
            );
        }
    };
    match serde_json::from_str::<Value>(&text) {
        Ok(raw) => (source(SourceStatus::Loaded), normalize(&raw)),
        Err(err) => (
            source(SourceStatus::Error(err.to_string())),
            Partial::default(),
        ),
    }
}

pub fn global_path(agent_dir: &Path) -> PathBuf {
    agent_dir.join("ext").join("computer-use.json")
}

pub fn project_path(cwd: &Path) -> PathBuf {
    cwd.join(".pi").join("ext").join("computer-use.json")
}

/// Merges defaults, the global file, the project file (when trusted), then env.
pub fn load(agent_dir: &Path, cwd: &Path, trusted: bool, env: Partial) -> Loaded {
    let mut config = Config::default();
    let mut sources = Vec::new();
    let (global, values) = read(&global_path(agent_dir));
    config.apply(&values);
    sources.push(global);
    let project = project_path(cwd);
    if trusted {
        let (source, values) = read(&project);
        config.apply(&values);
        sources.push(source);
    } else if project.exists() {
        sources.push(Source {
            path: project,
            status: SourceStatus::Untrusted,
        });
    }
    config.apply(&env);
    Loaded {
        config,
        sources,
        env,
    }
}

pub fn format_status(loaded: &Loaded) -> String {
    let state = |flag: bool| if flag { "enabled" } else { "disabled" };
    let config = &loaded.config;
    let mut lines = vec![
        "pi-computer-use configuration".to_owned(),
        format!("browser_use: {}", state(config.browser_use)),
        format!("managed_browser: {}", config.managed_browser.as_str()),
        format!(
            "headless: {} (when enabled, act_ui accepts only press and setText on accessibility refs)",
            state(config.headless)
        ),
        format!(
            "cursor_overlay: {} (pi-rust sends pointer input in the foreground, so the system cursor moves and no overlay is drawn)",
            state(config.cursor_overlay)
        ),
        String::new(),
        "Sources:".to_owned(),
    ];
    for source in &loaded.sources {
        let status = match &source.status {
            SourceStatus::Loaded => "loaded".to_owned(),
            SourceStatus::NotFound => "not found".to_owned(),
            SourceStatus::Untrusted => "ignored: project is not trusted".to_owned(),
            SourceStatus::Error(err) => format!("error: {err}"),
        };
        lines.push(format!("- {}: {status}", source.path.display()));
    }
    let env = loaded.env.keys().join(", ");
    let env = if env.is_empty() {
        "none".to_owned()
    } else {
        env
    };
    lines.push(format!("- env overrides: {env}"));
    lines.join("\n")
}

const BROWSER_NAMES: [&str; 10] = [
    "safari", "chrome", "chromium", "firefox", "arc", "brave", "edge", "vivaldi", "helium",
    "msedge",
];
const BROWSER_BUNDLES: [&str; 9] = [
    "com.apple.safari",
    "com.google.chrome",
    "org.chromium.chromium",
    "org.mozilla.firefox",
    "company.thebrowser.browser",
    "com.brave.browser",
    "com.microsoft.edgemac",
    "com.vivaldi.vivaldi",
    "net.imput.helium",
];

/// Known browser families that `browser_use: false` refuses.
pub fn is_browser(app: &str, bundle_id: Option<&str>) -> bool {
    let bundle = bundle_id.unwrap_or_default().to_lowercase();
    if BROWSER_BUNDLES.iter().any(|b| bundle.starts_with(b)) {
        return true;
    }
    let app = app.to_lowercase();
    app.split(|c: char| !c.is_alphanumeric())
        .any(|word| BROWSER_NAMES.contains(&word))
}
