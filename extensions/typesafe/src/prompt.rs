use pi_ext_sdk::prelude::*;

const PASTE_START: &str = "\x1b[200~";
const PASTE_END: &str = "\x1b[201~";

/// Applies one key to the hidden value. `Some(true)` submits, `Some(false)` cancels.
pub fn apply_key(value: &mut String, key: &Key) -> Option<bool> {
    match key {
        Key::Enter => return Some(true),
        Key::Escape | Key::Ctrl('c') => return Some(false),
        Key::Backspace => {
            value.pop();
        }
        Key::Char(c) => value.push(*c),
        Key::Text(text) => value.push_str(text),
        Key::Unknown(raw) => {
            let text = raw.replace(PASTE_START, "").replace(PASTE_END, "");
            value.extend(text.chars().filter(|c| !c.is_control()));
        }
        _ => {}
    }
    None
}

pub fn lines(label: &str, length: usize, width: usize) -> Vec<String> {
    let bullets = "•".repeat(length.min(width.saturating_sub(2)));
    vec![
        style::bold(&format!("{label} API key")),
        style::dim("Paste the key. Input is hidden. Enter saves, Esc cancels."),
        style::truncate(&format!("{bullets}{}", style::inverse(" ")), width),
    ]
}

/// Hidden key input in a panel; falls back to the plain input dialog when panels fail.
pub async fn prompt_for_key(host: &Host, label: &str) -> Result<Option<String>> {
    let mut width = 80;
    let panel = host.panel(lines(label, 0, width)).open().await;
    let mut panel = match panel {
        Ok(panel) => panel,
        Err(error) if error.is_no_ui() => return Err(error),
        Err(_) => {
            let title = format!("{label} API key (visible while typing)");
            return host
                .input(&title, Some("Paste the key, then press Enter"))
                .await;
        }
    };
    let mut value = String::new();
    loop {
        match panel.next_event().await {
            PanelEvent::Key(key) => match apply_key(&mut value, &key) {
                Some(true) => {
                    panel.close(Value::Null).await?;
                    return Ok(Some(value));
                }
                Some(false) => {
                    panel.close(Value::Null).await?;
                    return Ok(None);
                }
                None => panel.set_lines(lines(label, value.chars().count(), width))?,
            },
            PanelEvent::Resize(columns) => {
                width = columns;
                panel.set_lines(lines(label, value.chars().count(), width))?;
            }
            PanelEvent::Closed => return Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_hidden_value() {
        let mut value = String::new();
        assert_eq!(apply_key(&mut value, &Key::Char('a')), None);
        apply_key(&mut value, &Key::Text("bcd".into()));
        apply_key(&mut value, &Key::Backspace);
        apply_key(&mut value, &Key::parse("\x1b[200~xyz\x1b[201~"));
        assert_eq!(value, "abcxyz");
        assert_eq!(apply_key(&mut value, &Key::Char('q')), None);
        assert_eq!(apply_key(&mut value, &Key::Enter), Some(true));
        assert_eq!(apply_key(&mut value, &Key::Escape), Some(false));
        let shown = lines("TypeSafe", 4, 20);
        assert!(shown[2].contains("••••"));
        assert!(!shown.concat().contains("abc"));
    }
}
