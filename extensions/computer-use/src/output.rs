use pi_ext_sdk::{ToolResult, content};
use serde_json::{Value, json};

use crate::backend::Image;
use crate::state::{Observation, Store};

pub const MAX_BYTES: usize = 48 * 1024;
pub const MAX_LINES: usize = 2000;
pub const OUTPUT_PAGE: usize = 16 * 1024;
pub const TEXT_PAGE: usize = 4000;
pub const RENDER_DEPTH: usize = 12;
pub const RENDER_LINES: usize = 400;

fn floor_boundary(text: &str, mut index: usize) -> usize {
    index = index.min(text.len());
    while !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

/// Cuts text to the model bound; the full text goes to an `@o` ref.
pub fn bound(store: &mut Store, text: String) -> String {
    let line_cut = text.match_indices('\n').nth(MAX_LINES - 1).map(|(i, _)| i);
    let cut = line_cut.unwrap_or(text.len()).min(MAX_BYTES);
    if cut >= text.len() {
        return text;
    }
    let cut = floor_boundary(&text, cut);
    let head = text[..cut].to_owned();
    let reference = store.put_output(text);
    format!(
        "{head}\n\nOutput truncated at byte {cut}. Continue with read_text({{ ref: \"{reference}\", offset: {cut} }})."
    )
}

/// One fixed page of `@o` output; offsets are UTF-8 byte offsets.
pub fn output_page(text: &str, offset: usize) -> String {
    let start = floor_boundary(text, offset);
    let end = floor_boundary(text, start + OUTPUT_PAGE);
    let mut page = text[start..end].to_owned();
    if end < text.len() {
        page.push_str(&format!(
            "\n\nMore output remains. Continue with offset {end}."
        ));
    } else {
        page.push_str("\n\nEnd of output.");
    }
    page
}

/// One fixed page of element text; offsets count Unicode characters.
pub fn text_page(text: &str, offset: usize) -> String {
    let total = text.chars().count();
    let page: String = text.chars().skip(offset).take(TEXT_PAGE).collect();
    let end = (offset + TEXT_PAGE).min(total);
    if end < total {
        format!(
            "{page}\n\nShowing characters {offset}-{end} of {total}. Continue with offset {end}."
        )
    } else {
        format!("{page}\n\nEnd of text ({total} characters).")
    }
}

pub fn render(observation: &Observation) -> String {
    let (text, omitted) = observation.outline.root.render(RENDER_DEPTH, RENDER_LINES);
    if omitted == 0 {
        return text;
    }
    format!("{text}\n… {omitted} more nodes. Use search_ui or expand_ui.")
}

pub fn header(observation: &Observation) -> String {
    let target = &observation.target;
    let root = target
        .window_ref
        .as_deref()
        .map(|r| format!("{r} "))
        .unwrap_or_default();
    let image = if observation.capture.width > 0 {
        format!(
            " Image {}x{} (window-relative screenshot pixels).",
            observation.capture.width, observation.capture.height
        )
    } else {
        String::new()
    };
    format!(
        "stateId: {}\nroot: {root}{} — {}{image}",
        observation.capture.state_id, target.app, target.window_title
    )
}

pub fn details(tool: &str, observation: &Observation, rendered: &str, base: Option<&str>) -> Value {
    let mut details = json!({
        "tool": tool,
        "target": observation.target,
        "capture": observation.capture,
        "lookId": observation.outline.look_id,
        "view": "full",
        "renderedOutline": rendered,
        "outline": observation.outline,
    });
    if let Some(base) = base {
        details["baseStateId"] = json!(base);
    }
    details
}

/// Text plus optional image, with persisted details for branch replay.
pub fn observation_result(
    store: &mut Store,
    tool: &str,
    summary: &str,
    observation: &Observation,
    image: Option<Image>,
    base: Option<&str>,
) -> ToolResult {
    let rendered = render(observation);
    let text = bound(
        store,
        format!("{summary}\n{}\n\n{rendered}", header(observation)),
    );
    let mut blocks = vec![content::text(text)];
    if let Some(image) = image {
        blocks.push(content::image(image.base64, "image/jpeg"));
    }
    ToolResult::content(blocks).details(details(tool, observation, &rendered, base))
}
