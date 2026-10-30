//! Question walker on `ui.select` and `ui.input` for hosts without `ui.custom` panels.

use pi_ext_sdk::{Host, Result};

use crate::result::{AnswerKind, QuestionAnswer, QuestionnaireResult};
use crate::schema::{OTHER_LABEL, Question, QuestionOption};

const MULTI_SELECT_INSTRUCTIONS: &str = "Enter the numbers of all that apply, comma-separated (e.g. \"1,3\"). Add \"; custom answer\" to include your own text, or type only custom text.";
const CUSTOM_ANSWER_TITLE: &str = "Type your answer:";
const MULTI_SELECT_PLACEHOLDER: &str = "1,3; custom answer";
const MAX_PREVIEW_CHARS: usize = 600;

/// The two dialogs the walker needs; `Host` implements it, tests script it.
pub trait Dialogs {
    async fn select(&self, title: &str, options: &[String]) -> Result<Option<String>>;
    async fn input(&self, title: &str, placeholder: &str) -> Result<Option<String>>;
}

impl Dialogs for Host {
    async fn select(&self, title: &str, options: &[String]) -> Result<Option<String>> {
        Host::select(self, title, options).await
    }

    async fn input(&self, title: &str, placeholder: &str) -> Result<Option<String>> {
        Host::input(self, title, Some(placeholder)).await
    }
}

fn option_line(option: &QuestionOption, index: usize) -> String {
    format!("{}. {} — {}", index + 1, option.label, option.description)
}

fn parse_index(token: &str, count: usize) -> Option<usize> {
    let digits: String = token.chars().take_while(char::is_ascii_digit).collect();
    let n: usize = digits.parse().ok()?;
    (1..=count).contains(&n).then(|| n - 1)
}

fn is_number_token(token: &str) -> bool {
    let token = token.strip_suffix('.').unwrap_or(token);
    !token.is_empty() && token.chars().all(|c| c.is_ascii_digit())
}

fn preview_block(q: &Question) -> String {
    let blocks: Vec<String> = q
        .options
        .iter()
        .enumerate()
        .filter_map(|(i, o)| {
            let preview: String = o.preview()?.chars().take(MAX_PREVIEW_CHARS).collect();
            Some(format!("--- {}. {} preview ---\n{preview}", i + 1, o.label))
        })
        .collect();
    if blocks.is_empty() {
        String::new()
    } else {
        format!("\n\n{}", blocks.join("\n\n"))
    }
}

/// Dismissing any dialog cancels the questionnaire, as Esc does in the panel.
pub async fn walk<D: Dialogs>(ui: &D, questions: &[Question]) -> Result<QuestionnaireResult> {
    let mut answers = Vec::new();
    for (index, q) in questions.iter().enumerate() {
        let header = if q.header.is_empty() {
            String::new()
        } else {
            format!("[{}] ", q.header)
        };
        let answer = if q.is_multi() {
            ask_multi(ui, q, index, &header).await?
        } else {
            ask_single(ui, q, index, &header).await?
        };
        match answer {
            Some(answer) => answers.push(answer),
            None => {
                return Ok(QuestionnaireResult {
                    answers,
                    cancelled: true,
                    ..QuestionnaireResult::default()
                });
            }
        }
    }
    Ok(QuestionnaireResult {
        answers,
        ..QuestionnaireResult::default()
    })
}

async fn ask_single<D: Dialogs>(
    ui: &D,
    q: &Question,
    index: usize,
    header: &str,
) -> Result<Option<QuestionAnswer>> {
    let mut lines: Vec<String> = q
        .options
        .iter()
        .enumerate()
        .map(|(i, o)| option_line(o, i))
        .collect();
    let listing = lines.join("\n");
    lines.push(format!("{}. {OTHER_LABEL}", q.options.len() + 1));
    let title = format!("{header}{}{}", q.question, preview_block(q));
    loop {
        let Some(chosen) = ui.select(&title, &lines).await? else {
            return Ok(None);
        };
        let reply = chosen.trim();
        if reply.is_empty() {
            return Ok(None);
        }
        let picked = lines
            .iter()
            .position(|line| *line == chosen)
            .or_else(|| {
                if is_number_token(reply) {
                    parse_index(reply, lines.len())
                } else {
                    None
                }
            })
            .or_else(|| q.options.iter().position(|o| o.label == reply));
        let Some(picked) = picked else {
            return Ok(Some(QuestionAnswer::new(
                index,
                &q.question,
                AnswerKind::Custom,
                Some(reply.to_owned()),
            )));
        };
        if let Some(option) = q.options.get(picked) {
            let mut answer = QuestionAnswer::new(
                index,
                &q.question,
                AnswerKind::Option,
                Some(option.label.clone()),
            );
            answer.preview = option.preview().map(str::to_owned);
            return Ok(Some(answer));
        }
        let prompt = format!(
            "{header}{}\n\n{listing}\n\n{CUSTOM_ANSWER_TITLE}",
            q.question
        );
        match ui.input(&prompt, "").await? {
            Some(typed) if !typed.trim().is_empty() => {
                return Ok(Some(QuestionAnswer::new(
                    index,
                    &q.question,
                    AnswerKind::Custom,
                    Some(typed),
                )));
            }
            _ => continue,
        }
    }
}

async fn ask_multi<D: Dialogs>(
    ui: &D,
    q: &Question,
    index: usize,
    header: &str,
) -> Result<Option<QuestionAnswer>> {
    let listing: Vec<String> = q
        .options
        .iter()
        .enumerate()
        .map(|(i, o)| option_line(o, i))
        .collect();
    let title = format!(
        "{header}{}\n\n{}\n\n{MULTI_SELECT_INSTRUCTIONS}",
        q.question,
        listing.join("\n")
    );
    let Some(value) = ui.input(&title, MULTI_SELECT_PLACEHOLDER).await? else {
        return Ok(None);
    };
    let multi = |answer: Option<String>, selected: Vec<String>| {
        let mut out = QuestionAnswer::new(index, &q.question, AnswerKind::Multi, answer);
        out.selected = Some(selected);
        Some(out)
    };
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(multi(None, Vec::new()));
    }
    let (selection, custom) = match trimmed.split_once(';') {
        Some((selection, custom)) => (selection.trim(), custom.trim()),
        None => (trimmed, ""),
    };
    let tokens: Vec<&str> = selection
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|t| !t.is_empty())
        .collect();
    let indices: Vec<Option<usize>> = tokens
        .iter()
        .map(|t| {
            if is_number_token(t) {
                parse_index(t, q.options.len())
            } else {
                None
            }
        })
        .collect();
    if selection.is_empty() || indices.iter().all(Option::is_some) {
        let mut selected: Vec<String> = Vec::new();
        for i in indices.into_iter().flatten() {
            let label = q.options[i].label.clone();
            if !selected.contains(&label) {
                selected.push(label);
            }
        }
        let custom = (!custom.is_empty()).then(|| custom.to_owned());
        return Ok(multi(custom, selected));
    }
    Ok(multi(Some(trimmed.to_owned()), Vec::new()))
}
