use std::collections::VecDeque;
use std::sync::Mutex;

use pi_ext_sdk::Result;

use super::{question, questions};
use crate::fallback::{Dialogs, walk};
use crate::result::AnswerKind;

#[derive(Default)]
struct Script {
    replies: Mutex<VecDeque<Option<String>>>,
    titles: Mutex<Vec<String>>,
}

impl Script {
    fn new(replies: &[Option<&str>]) -> Self {
        let replies = replies.iter().map(|r| r.map(str::to_owned)).collect();
        Self {
            replies: Mutex::new(replies),
            titles: Mutex::default(),
        }
    }

    fn next(&self, title: &str) -> Result<Option<String>> {
        self.titles.lock().unwrap().push(title.to_owned());
        Ok(self.replies.lock().unwrap().pop_front().flatten())
    }
}

impl Dialogs for Script {
    async fn select(&self, title: &str, _options: &[String]) -> Result<Option<String>> {
        self.next(title)
    }

    async fn input(&self, title: &str, _placeholder: &str) -> Result<Option<String>> {
        self.next(title)
    }
}

#[tokio::test]
async fn walks_single_and_multi_questions() {
    let qs = questions(vec![
        question("Db?", "DB", &["Pg", "Lite"], false),
        question("Own?", "Own", &["A", "B"], false),
        question("Many?", "M", &["X", "Y", "Z"], true),
        question("Text?", "T", &["P", "Q"], true),
    ]);
    let script = Script::new(&[
        Some("2. Lite — Lite description"),
        Some("3. Type something."),
        Some(" "),
        Some("3"),
        Some("mine"),
        Some("3, 1; extra"),
        Some("free words"),
    ]);
    let result = walk(&script, &qs).await.unwrap();
    assert!(!result.cancelled);
    assert_eq!(result.answers[0].answer.as_deref(), Some("Lite"));
    assert_eq!(result.answers[1].kind, AnswerKind::Custom);
    assert_eq!(result.answers[1].answer.as_deref(), Some("mine"));
    assert_eq!(result.answers[2].selected.clone().unwrap(), ["Z", "X"]);
    assert_eq!(result.answers[2].answer.as_deref(), Some("extra"));
    assert_eq!(result.answers[3].answer.as_deref(), Some("free words"));
    assert!(script.titles.lock().unwrap()[0].starts_with("[DB] Db?"));
}

#[tokio::test]
async fn dismissing_cancels() {
    let qs = questions(vec![question("Db?", "DB", &["Pg", "Lite"], false)]);
    let result = walk(&Script::new(&[None]), &qs).await.unwrap();
    assert!(result.cancelled);
    let typed = walk(&Script::new(&[Some("Pg")]), &qs).await.unwrap();
    assert_eq!(typed.answers[0].kind, AnswerKind::Option);
}
