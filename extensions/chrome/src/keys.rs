use anyhow::{Result, bail};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::page::Page;

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct Modifiers {
    pub shift_key: bool,
    pub ctrl_key: bool,
    pub alt_key: bool,
    pub meta_key: bool,
}

impl Modifiers {
    pub fn bits(&self) -> u8 {
        u8::from(self.alt_key)
            | (u8::from(self.ctrl_key) << 1)
            | (u8::from(self.meta_key) << 2)
            | (u8::from(self.shift_key) << 3)
    }
}

pub fn key_event(key: &str, modifiers: Modifiers) -> Result<Value> {
    let named =
        |code: &str, vk: u32, text: &str| (key.to_owned(), code.to_owned(), vk, text.to_owned());
    let (key, code, vk, text) = match key {
        "Enter" | "Return" => named("Enter", 13, "\r"),
        "Tab" => named("Tab", 9, ""),
        "Escape" | "Esc" => named("Escape", 27, ""),
        "Backspace" => named("Backspace", 8, ""),
        "Delete" => named("Delete", 46, ""),
        "ArrowUp" | "ArrowDown" | "ArrowLeft" | "ArrowRight" => {
            let vk = match key {
                "ArrowLeft" => 37,
                "ArrowUp" => 38,
                "ArrowRight" => 39,
                _ => 40,
            };
            named(key, vk, "")
        }
        "Home" => named("Home", 36, ""),
        "End" => named("End", 35, ""),
        "PageUp" => named("PageUp", 33, ""),
        "PageDown" => named("PageDown", 34, ""),
        " " | "Space" => (" ".to_owned(), "Space".to_owned(), 32, " ".to_owned()),
        _ => {
            let mut chars = key.chars();
            let (Some(ch), None) = (chars.next(), chars.next()) else {
                bail!("Unknown key {key:?}. Use a key name such as Enter or one character.");
            };
            char_key(ch, modifiers.shift_key)
        }
    };
    let typed =
        !text.is_empty() && !(modifiers.ctrl_key || modifiers.meta_key || modifiers.alt_key);
    let mut event = json!({
        "key": key, "code": code, "windowsVirtualKeyCode": vk,
        "nativeVirtualKeyCode": vk, "modifiers": modifiers.bits(),
    });
    if typed {
        event["text"] = json!(text);
        event["unmodifiedText"] = json!(text);
    }
    Ok(event)
}

fn char_key(ch: char, shift: bool) -> (String, String, u32, String) {
    const SHIFTED: &[(char, char)] = &[
        ('1', '!'),
        ('2', '@'),
        ('3', '#'),
        ('4', '$'),
        ('5', '%'),
        ('6', '^'),
        ('7', '&'),
        ('8', '*'),
        ('9', '('),
        ('0', ')'),
        ('-', '_'),
        ('=', '+'),
        ('[', '{'),
        (']', '}'),
        (';', ':'),
        ('\'', '"'),
        (',', '<'),
        ('.', '>'),
        ('/', '?'),
    ];
    let shifted = match (shift, ch.is_ascii_alphabetic()) {
        (true, true) => ch.to_ascii_uppercase(),
        (true, false) => SHIFTED
            .iter()
            .find(|(c, _)| *c == ch)
            .map_or(ch, |(_, s)| *s),
        _ => ch,
    };
    let (code, vk) = if ch.is_ascii_alphabetic() {
        (
            format!("Key{}", ch.to_ascii_uppercase()),
            ch.to_ascii_uppercase() as u32,
        )
    } else if ch.is_ascii_digit() {
        (format!("Digit{ch}"), ch as u32)
    } else {
        (String::new(), 0)
    };
    (shifted.to_string(), code, vk, shifted.to_string())
}

pub async fn press(page: &Page, key: &str, modifiers: Modifiers) -> Result<()> {
    let event = key_event(key, modifiers)?;
    let mut down = event.clone();
    down["type"] = json!(if event.get("text").is_some() {
        "keyDown"
    } else {
        "rawKeyDown"
    });
    page.call("Input.dispatchKeyEvent", down).await?;
    let mut up = event;
    up["type"] = json!("keyUp");
    up.as_object_mut().map(|o| o.remove("text"));
    page.call("Input.dispatchKeyEvent", up).await.map(drop)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_named_and_char_keys() {
        let enter = key_event("Enter", Modifiers::default()).unwrap();
        assert_eq!(
            (
                enter["windowsVirtualKeyCode"].as_u64(),
                enter["text"].as_str()
            ),
            (Some(13), Some("\r"))
        );
        let shift = Modifiers {
            shift_key: true,
            ..Modifiers::default()
        };
        let a = key_event("a", shift).unwrap();
        assert_eq!(
            (
                a["key"].as_str(),
                a["code"].as_str(),
                a["modifiers"].as_u64()
            ),
            (Some("A"), Some("KeyA"), Some(8))
        );
        assert_eq!(key_event("1", shift).unwrap()["text"], "!");
        let ctrl = Modifiers {
            ctrl_key: true,
            ..Modifiers::default()
        };
        assert!(key_event("c", ctrl).unwrap().get("text").is_none());
        assert!(key_event("Hyper", Modifiers::default()).is_err());
    }
}
