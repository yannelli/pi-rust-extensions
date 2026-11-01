//! The layout editor of `/statusline`: move, add, and remove modules across rows.

use serde_json::{Value, json};

use crate::builtin::BuiltinStatusLine;
use crate::editor_items::choice;
use crate::list::SettingItem;
use crate::segments::{ItemKind, Segment};

pub fn layout_values(builtin: &BuiltinStatusLine) -> Vec<Vec<Value>> {
    builtin
        .lines
        .iter()
        .map(|line| {
            line.iter()
                .map(|item| {
                    let mut value = match &item.kind {
                        ItemKind::Segment(segment) => json!({"segment": segment.name()}),
                        ItemKind::Text(text) => json!({"text": text}),
                    };
                    if let Some(color) = &item.color {
                        value["color"] = json!(color);
                    }
                    if let Some(icon) = &item.icon {
                        value["icon"] = json!(icon);
                    }
                    if let Some(foreground) = &item.foreground {
                        value["foreground"] = json!(foreground);
                    }
                    if let Some(background) = &item.background {
                        value["background"] = json!(background);
                    }
                    value
                })
                .collect()
        })
        .collect()
}

pub fn layout_items(lines: &[Vec<Value>]) -> Vec<SettingItem> {
    let mut items = Vec::new();
    let modules: Vec<&str> = Segment::ALL.iter().map(|segment| segment.name()).collect();
    for (row, line) in lines.iter().enumerate() {
        for (index, item) in line.iter().enumerate() {
            let name = item
                .as_str()
                .or_else(|| item["segment"].as_str())
                .or_else(|| item["text"].as_str())
                .unwrap_or("?");
            items.push(choice(
                &format!("{row}:{index}"),
                &format!("{}.{} {name}", row + 1, index + 1),
                "Move within a row, move to another row, or remove this module.",
                "edit",
                &["earlier", "later", "previous row", "next row", "remove"],
            ));
        }
        items.push(choice(
            &format!("add:{row}"),
            &format!("Add to row {}", row + 1),
            "Fill separates left and right groups in spread alignment.",
            "choose",
            &modules,
        ));
    }
    for (id, label, description) in [
        ("row", "Add row", "Adds an empty row."),
        (
            "apply",
            "Apply layout",
            "Saves the layout. Esc discards it.",
        ),
    ] {
        items.push(choice(id, label, description, "", &[]));
    }
    items
}

pub fn change_layout(lines: &mut Vec<Vec<Value>>, id: &str, selected: &str) {
    if id == "row" {
        lines.push(Vec::new());
        return;
    }
    let Some((row, index)) = id.split_once(':') else {
        return;
    };
    if row == "add" {
        if let Ok(row) = index.parse::<usize>()
            && let Some(line) = lines.get_mut(row)
            && Segment::from_name(selected).is_some()
        {
            line.push(json!(selected));
        }
        return;
    }
    let (Ok(row), Ok(index)) = (row.parse::<usize>(), index.parse::<usize>()) else {
        return;
    };
    let Some(line) = lines.get_mut(row).filter(|line| index < line.len()) else {
        return;
    };
    match selected {
        "remove" => {
            line.remove(index);
        }
        "earlier" if index > 0 => line.swap(index, index - 1),
        "later" if index + 1 < line.len() => line.swap(index, index + 1),
        "previous row" if row > 0 => {
            let item = line.remove(index);
            lines[row - 1].push(item);
        }
        "next row" => {
            let item = line.remove(index);
            if row + 1 == lines.len() {
                lines.push(Vec::new());
            }
            lines[row + 1].push(item);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_actions_preserve_objects_and_can_remove_all_modules() {
        let pill = json!({"segment": "model", "color": "#ffffff", "icon": "M", "foreground": "#000", "background": "accent"});
        let mut lines = vec![vec![json!("dir"), pill.clone()]];
        change_layout(&mut lines, "0:1", "earlier");
        assert_eq!(lines[0][0], pill);
        change_layout(&mut lines, "0:0", "next row");
        assert_eq!(lines[1], vec![pill.clone()]);
        change_layout(&mut lines, "add:1", "contextHeadroom");
        change_layout(&mut lines, "1:0", "later");
        assert_eq!(lines[1], vec![json!("contextHeadroom"), pill]);
        change_layout(&mut lines, "1:0", "previous row");
        assert_eq!(lines[0], vec![json!("dir"), json!("contextHeadroom")]);
        change_layout(&mut lines, "1:0", "remove");
        change_layout(&mut lines, "0:1", "remove");
        change_layout(&mut lines, "0:0", "remove");
        assert!(lines.iter().all(Vec::is_empty));
    }
}
