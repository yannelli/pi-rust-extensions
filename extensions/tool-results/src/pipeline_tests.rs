use pi_ext_sdk::content;
use serde_json::json;

use super::*;
use crate::config::{Config, ErrorMode, JsonFormat};

fn text_of(outcome: &Outcome) -> &str {
    outcome.content[0]["text"].as_str().unwrap()
}

#[test]
fn cleans_terminal_output() {
    let warn = "warning: unused variable `x` in src/main.rs";
    let raw = format!(
        "\x1b[32mok\x1b[0m   \nstep 1%\rstep 100%\n\n\n\n{warn}\n{warn}\n{warn}\n{warn}\ndone\0"
    );
    let out = process(&[content::text(&raw)], &Settings::default(), "tr-1", 200).unwrap();
    assert_eq!(
        text_of(&out),
        format!("ok\nstep 100%\n\n{warn}\n[previous line repeated 3 more times]\ndone")
    );
    assert_eq!(
        out.applied,
        ["ansi", "progress", "control", "dedupe", "whitespace"]
    );
    assert!(out.after < out.before);
    assert_eq!(out.stored, None);
}

#[test]
fn leaves_clean_text_and_images_alone() {
    let blocks = [
        content::text("all good"),
        content::image("AAAA", "image/png"),
    ];
    assert_eq!(process(&blocks, &Settings::default(), "tr-1", 200), None);
    assert_eq!(
        process(
            &[content::image("AAAA", "image/png")],
            &Settings::default(),
            "x",
            1
        ),
        None
    );
}

#[test]
fn images_survive_a_change() {
    let blocks = [
        content::text("a   \n\n\n\nb"),
        content::image("AAAA", "image/png"),
    ];
    let out = process(&blocks, &Settings::default(), "tr-1", 200).unwrap();
    assert_eq!(out.content[0]["text"], "a\n\nb");
    assert_eq!(out.content[1], content::image("AAAA", "image/png"));
}

#[test]
fn converts_json_blocks_and_skips_line_transforms() {
    let raw =
        serde_json::to_string_pretty(&json!([{ "id": 1, "ok": true }, { "id": 2, "ok": false }]))
            .unwrap();
    let out = process(&[content::text(&raw)], &Settings::default(), "tr-1", 200).unwrap();
    assert_eq!(text_of(&out), "[2]{id,ok}:\n  1,true\n  2,false");
    assert_eq!(out.applied, ["json:toon"]);
    let off = Settings {
        json: JsonFormat::Off,
        ..Settings::default()
    };
    let same = "[\n  0,\n  0,\n  0,\n  0\n]";
    assert_eq!(process(&[content::text(same)], &off, "tr-1", 200), None);
}

#[test]
fn truncates_large_results_with_a_notice() {
    let raw: String = (1..=5000).map(|n| format!("line {n}\n")).collect();
    let s = Settings {
        threshold: 1000,
        head_chars: 100,
        tail_chars: 50,
        ..Settings::default()
    };
    let out = process(&[content::text(&raw)], &s, "tr-abc", 150).unwrap();
    let text = text_of(&out);
    assert!(out.truncated);
    assert!(text.starts_with("line 1\nline 2\n"), "{text}");
    assert!(text.contains("characters omitted ...]\n\n"), "{text}");
    assert!(
        text.contains("line 5000\n\nResult truncated (48.9k characters, 5,000 lines)."),
        "{text}"
    );
    assert!(text.ends_with(
        "Use `tool_result_get` with result_id \"tr-abc\" to read a line range, search for a pattern \
         and return matches with about 150 characters of context, or return it in full."
    ));
    assert_eq!(out.stored.as_deref(), Some(raw.as_str()));
    assert!(out.after < 500, "{}", out.after);
}

#[test]
fn stores_results_with_elided_parts() {
    let raw = format!("key: {}", "aZ09+/QxbcDEfgHIjkLMnoPQrsTUvwXY".repeat(16));
    let out = process(&[content::text(&raw)], &Settings::default(), "tr-9", 200).unwrap();
    assert!(
        text_of(&out)
            .starts_with("key: [base64 elided: 512 characters]\n\n[Some parts were elided.")
    );
    assert_eq!(out.stored.as_deref(), Some(raw.as_str()));
    let keep = Settings {
        keep_elided: false,
        ..Settings::default()
    };
    let out = process(&[content::text(&raw)], &keep, "tr-9", 200).unwrap();
    assert_eq!(text_of(&out), "key: [base64 elided: 512 characters]");
    assert_eq!(out.stored, None);
}

#[test]
fn error_results_keep_only_cleanup() {
    let s = Settings::default().for_error().unwrap();
    let raw = format!("\x1b[31mfailed\x1b[0m\n{}", "same\n".repeat(5));
    let out = process(&[content::text(&raw)], &s, "tr-1", 200).unwrap();
    assert_eq!(
        text_of(&out),
        format!("failed\n{}", "same\n".repeat(5).trim_end())
    );
    let off = Settings {
        errors: ErrorMode::Off,
        ..Settings::default()
    };
    assert_eq!(off.for_error(), None);
    let all = Settings {
        errors: ErrorMode::All,
        ..Settings::default()
    };
    assert_eq!(all.for_error(), Some(all.clone()));
}

#[test]
fn read_rule_keeps_file_text() {
    let config = Config::default();
    let s = config.for_tool("read");
    let raw = "fn main() {   \n\n\n\n}\n\x1b[0m";
    assert_eq!(process(&[content::text(raw)], &s, "tr-1", 200), None);
    assert!(s.truncate);
}
