//! One entry of the `segments` setting.

use serde_json::Value;

use crate::segments::Segment;

/// One entry of `segments`: a segment, or literal `text`, with an optional color and icon.
/// `foreground` and `background` color this badge only: a pi theme token, `#rrggbb`, or `#rgb`.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub kind: ItemKind,
    pub color: Option<String>,
    pub icon: Option<String>,
    pub foreground: Option<String>,
    pub background: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ItemKind {
    Segment(Segment),
    Text(String),
}

impl Item {
    pub fn segment(segment: Segment) -> Self {
        Self {
            kind: ItemKind::Segment(segment),
            color: None,
            icon: None,
            foreground: None,
            background: None,
        }
    }

    /// `"model"`, `{"segment": "model", "color": "#hex", "icon": "◆", "foreground": "text", "background": "#hex"}`,
    /// or `{"text": "λ"}`.
    pub fn parse(value: &Value) -> Option<Self> {
        if let Some(name) = value.as_str() {
            return Segment::from_name(name).map(Self::segment);
        }
        let object = value.as_object()?;
        let field = |key: &str| object.get(key).and_then(Value::as_str).map(str::to_string);
        let kind = match (field("segment"), field("text")) {
            (Some(name), _) => ItemKind::Segment(Segment::from_name(&name)?),
            (None, Some(text)) => ItemKind::Text(text),
            (None, None) => return None,
        };
        Some(Self {
            kind,
            color: field("color"),
            icon: field("icon"),
            foreground: field("foreground"),
            background: field("background"),
        })
    }

    pub fn is_fill(&self) -> bool {
        self.kind == ItemKind::Segment(Segment::Fill)
    }
}
