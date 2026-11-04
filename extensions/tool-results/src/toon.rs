//! TOON encoder (Token-Oriented Object Notation, spec v4.4, comma delimiter, 2-space indent).
//! <https://github.com/toon-format/spec>

use serde_json::{Map, Number, Value};

pub fn encode(value: &Value) -> String {
    let mut out = Vec::new();
    match value {
        Value::Object(map) if map.is_empty() => {}
        Value::Object(map) => match columns(&map.values().collect::<Vec<_>>(), map.len() >= 2) {
            Some(fields) => keyed(None, map, &fields, 0, &mut out),
            None => fields(map, 0, &mut out),
        },
        Value::Array(items) => array(None, items, 0, true, &mut out),
        other => out.push(primitive(other)),
    }
    out.join("\n")
}

/// A header field: a leaf column, or a nested field group of uniform objects.
#[derive(Debug, PartialEq)]
enum Column {
    Leaf(String),
    Group(String, Vec<Column>),
}

fn indent(depth: usize) -> String {
    "  ".repeat(depth)
}

fn fields(map: &Map<String, Value>, depth: usize, out: &mut Vec<String>) {
    for (key, value) in map {
        field(key, value, depth, out);
    }
}

fn field(key: &str, value: &Value, depth: usize, out: &mut Vec<String>) {
    let (pad, name) = (indent(depth), encode_key(key));
    match value {
        Value::Object(map) if map.is_empty() => out.push(format!("{pad}{name}:")),
        Value::Object(map) => {
            let values: Vec<&Value> = map.values().collect();
            match columns(&values, map.len() >= 2) {
                Some(cols) => keyed(Some(&name), map, &cols, depth, out),
                None => {
                    out.push(format!("{pad}{name}:"));
                    fields(map, depth + 1, out);
                }
            }
        }
        Value::Array(items) => array(Some(&name), items, depth, true, out),
        other => out.push(format!("{pad}{name}: {}", primitive(other))),
    }
}

/// `tabular` is false for keyless arrays inside list items, which use list form (§9.4).
fn array(name: Option<&str>, items: &[Value], depth: usize, tabular: bool, out: &mut Vec<String>) {
    let (pad, name) = (indent(depth), name.unwrap_or(""));
    let n = items.len();
    if items.is_empty() {
        out.push(match name {
            "" => format!("{pad}[]"),
            _ => format!("{pad}{name}: []"),
        });
        return;
    }
    if items.iter().all(is_primitive) {
        out.push(format!("{pad}{name}[{n}]: {}", inline(items)));
        return;
    }
    let refs: Vec<&Value> = items.iter().collect();
    if let Some(cols) = tabular.then(|| columns(&refs, true)).flatten() {
        out.push(format!("{pad}{name}[{n}]{{{}}}:", header(&cols)));
        for item in items {
            out.push(format!(
                "{}{}",
                indent(depth + 1),
                row(item, &cols).join(",")
            ));
        }
        return;
    }
    out.push(format!("{pad}{name}[{n}]:"));
    for item in items {
        list_item(item, depth + 1, out);
    }
}

fn list_item(item: &Value, depth: usize, out: &mut Vec<String>) {
    let pad = indent(depth);
    match item {
        Value::Array(inner) if inner.is_empty() => out.push(format!("{pad}- [0]:")),
        Value::Array(inner) if inner.iter().all(is_primitive) => {
            out.push(format!("{pad}- [{}]: {}", inner.len(), inline(inner)));
        }
        Value::Array(inner) => {
            out.push(format!("{pad}- [{}]:", inner.len()));
            for nested in inner {
                list_item(nested, depth + 1, out);
            }
        }
        Value::Object(map) if map.is_empty() => out.push(format!("{pad}-")),
        Value::Object(map) => {
            let start = out.len();
            fields(map, depth + 1, out);
            let first = out[start].split_off(indent(depth + 1).len());
            out[start] = format!("{pad}- {first}");
        }
        other => out.push(format!("{pad}- {}", primitive(other))),
    }
}

fn keyed(
    name: Option<&str>,
    map: &Map<String, Value>,
    cols: &[Column],
    depth: usize,
    out: &mut Vec<String>,
) {
    let name = name.unwrap_or("");
    out.push(format!(
        "{}{name}[{}:]{{{}}}:",
        indent(depth),
        map.len(),
        header(cols)
    ));
    for (key, value) in map {
        let cells = row(value, cols).join(",");
        out.push(format!("{}{}: {cells}", indent(depth + 1), encode_key(key)));
    }
}

/// The columns when every value is a non-empty object with one key set and each column is
/// all primitives or uniform nested objects (§9.3); `None` otherwise.
fn columns(values: &[&Value], enough: bool) -> Option<Vec<Column>> {
    if !enough || values.is_empty() {
        return None;
    }
    let maps: Vec<&Map<String, Value>> = values
        .iter()
        .map(|v| v.as_object())
        .collect::<Option<_>>()?;
    let first = maps[0];
    if first.is_empty()
        || maps
            .iter()
            .any(|m| m.len() != first.len() || !first.keys().all(|k| m.contains_key(k)))
    {
        return None;
    }
    first
        .keys()
        .map(|key| {
            let column: Vec<&Value> = maps.iter().map(|m| &m[key]).collect();
            if column.iter().all(|v| is_primitive(v)) {
                Some(Column::Leaf(key.clone()))
            } else {
                columns(&column, true).map(|sub| Column::Group(key.clone(), sub))
            }
        })
        .collect()
}

fn header(cols: &[Column]) -> String {
    let parts: Vec<String> = cols
        .iter()
        .map(|col| match col {
            Column::Leaf(key) => encode_key(key),
            Column::Group(key, sub) => format!("{}{{{}}}", encode_key(key), header(sub)),
        })
        .collect();
    parts.join(",")
}

fn row(value: &Value, cols: &[Column]) -> Vec<String> {
    let mut cells = Vec::new();
    for col in cols {
        match col {
            Column::Leaf(key) => cells.push(primitive(&value[key.as_str()])),
            Column::Group(key, sub) => cells.extend(row(&value[key.as_str()], sub)),
        }
    }
    cells
}

fn inline(items: &[Value]) -> String {
    items.iter().map(primitive).collect::<Vec<_>>().join(",")
}

fn is_primitive(value: &Value) -> bool {
    !matches!(value, Value::Array(_) | Value::Object(_))
}

fn primitive(value: &Value) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => number(n),
        Value::String(s) => string(s),
        Value::Array(_) | Value::Object(_) => String::new(),
    }
}

/// Canonical decimal in `[1e-6, 1e21)`, exponent form outside it (§2).
pub fn number(n: &Number) -> String {
    if n.is_i64() || n.is_u64() {
        return n.to_string();
    }
    let Some(f) = n.as_f64().filter(|f| f.is_finite()) else {
        return "null".into();
    };
    if f == 0.0 {
        return "0".into();
    }
    if (1e-6..1e21).contains(&f.abs()) {
        return f.to_string();
    }
    let text = format!("{f:e}");
    match text.split_once('e') {
        Some((m, e)) if !e.starts_with('-') => format!("{m}e+{e}"),
        _ => text,
    }
}

fn numeric_like(s: &str) -> bool {
    let s = s.strip_prefix(['+', '-']).unwrap_or(s);
    let (mantissa, exponent) = match s.find(['e', 'E']) {
        Some(at) => (&s[..at], Some(&s[at + 1..])),
        None => (s, None),
    };
    let digits = |t: &str| !t.is_empty() && t.bytes().all(|b| b.is_ascii_digit());
    let mantissa_ok = match mantissa.split_once('.') {
        Some((int, frac)) => digits(int) && digits(frac),
        None => digits(mantissa),
    };
    let exponent_ok = exponent.is_none_or(|e| digits(e.strip_prefix(['+', '-']).unwrap_or(e)));
    mantissa_ok && exponent_ok
}

/// Quotes per §7.2 with the comma delimiter.
pub fn string(s: &str) -> String {
    let quote = s.is_empty()
        || s.starts_with([' ', '\t'])
        || s.ends_with([' ', '\t'])
        || matches!(s, "true" | "false" | "null")
        || numeric_like(s)
        || s.contains([':', '"', '\\', '[', ']', '{', '}', ','])
        || s.chars().any(|c| (c as u32) < 0x20)
        || s.starts_with(['-', '#']);
    if quote { quoted(s) } else { s.to_owned() }
}

fn quoted(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Keys stay bare when they match `^[A-Za-z_][A-Za-z0-9_.]*$` (§7.3).
pub fn encode_key(key: &str) -> String {
    let mut chars = key.chars();
    let bare = chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.');
    if bare { key.to_owned() } else { quoted(key) }
}

#[cfg(test)]
#[path = "toon_tests.rs"]
mod tests;
