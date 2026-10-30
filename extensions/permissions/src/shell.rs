//! Splits a bash script into simple commands for rule matching.

pub use crate::wrappers::{is_assignment, raw_text, strip_wrappers, values};

#[derive(Debug, Clone, PartialEq)]
pub struct Word {
    /// The text as written, quotes included.
    pub raw: String,
    /// The text after quote removal.
    pub value: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Script {
    /// Every simple command, including commands inside `$(...)`, backticks, and subshells.
    pub commands: Vec<Vec<Word>>,
    /// A linear chain of plain words joined by `&&`, `||`, `;`, `|` (the Codex split condition).
    pub plain: bool,
    /// False when `&&`, `||`, or `|` has no command after it.
    pub complete: bool,
}

const OPERATORS: &[char] = &[';', '&', '|', '(', ')', '<', '>', '\n'];

pub fn parse(script: &str) -> Script {
    let mut p = Parser {
        chars: script.chars().collect(),
        pos: 0,
        out: Script {
            commands: Vec::new(),
            plain: true,
            complete: true,
        },
        current: Vec::new(),
        pending: false,
    };
    p.run();
    p.out
}

struct Parser {
    chars: Vec<char>,
    pos: usize,
    out: Script,
    current: Vec<Word>,
    pending: bool,
}

impl Parser {
    fn peek(&self, ahead: usize) -> Option<char> {
        self.chars.get(self.pos + ahead).copied()
    }

    fn run(&mut self) {
        loop {
            while matches!(self.peek(0), Some(' ' | '\t')) {
                self.pos += 1;
            }
            let Some(c) = self.peek(0) else { break };
            match c {
                '\n' | ';' => self.end(1, false),
                '&' if self.peek(1) == Some('&') => self.end(2, true),
                '&' if self.peek(1) == Some('>') => self.redirect(),
                '&' => self.end(1, false),
                '|' if matches!(self.peek(1), Some('|' | '&')) => self.end(2, true),
                '|' => self.end(1, true),
                '(' | ')' => {
                    self.out.plain = false;
                    self.end(1, false);
                }
                '<' | '>' => self.redirect(),
                '#' => {
                    while self.peek(0).is_some_and(|c| c != '\n') {
                        self.pos += 1;
                    }
                }
                _ => {
                    let word = self.word();
                    let fd = !word.raw.is_empty() && word.raw.chars().all(|c| c.is_ascii_digit());
                    if fd && matches!(self.peek(0), Some('<' | '>')) {
                        continue;
                    }
                    self.pending = false;
                    self.current.push(word);
                }
            }
        }
        self.end(0, false);
        if self.pending {
            self.out.complete = false;
        }
    }

    fn end(&mut self, skip: usize, chain: bool) {
        self.pos += skip;
        if chain {
            self.pending = true;
        }
        let mut words = std::mem::take(&mut self.current);
        if words.first().is_some_and(|w| is_assignment(&w.raw)) {
            self.out.plain = false;
        }
        while let Some(first) = words.first() {
            match first.raw.as_str() {
                "if" | "then" | "elif" | "else" | "do" | "while" | "until" | "!" | "{" | "}"
                | "fi" | "done" | "esac" => {
                    self.out.plain = false;
                    words.remove(0);
                }
                "for" | "case" | "select" | "function" => {
                    self.out.plain = false;
                    words.clear();
                }
                _ => break,
            }
        }
        if !words.is_empty() {
            self.out.commands.push(words);
        }
    }

    fn redirect(&mut self) {
        self.out.plain = false;
        while matches!(self.peek(0), Some('<' | '>' | '&')) {
            self.pos += 1;
        }
        if self.peek(0).is_some_and(|c| c.is_ascii_digit() || c == '-') {
            while self.peek(0).is_some_and(|c| c.is_ascii_digit() || c == '-') {
                self.pos += 1;
            }
            return;
        }
        while matches!(self.peek(0), Some(' ' | '\t')) {
            self.pos += 1;
        }
        if self.peek(0).is_some_and(|c| !OPERATORS.contains(&c)) {
            self.word();
        }
    }

    fn word(&mut self) -> Word {
        let start = self.pos;
        let mut value = String::new();
        while let Some(c) = self.peek(0) {
            if c == ' ' || c == '\t' || OPERATORS.contains(&c) {
                break;
            }
            self.pos += 1;
            match c {
                '\\' => {
                    if let Some(next) = self.peek(0) {
                        self.pos += 1;
                        if next != '\n' {
                            value.push(next);
                        }
                    }
                }
                '\'' => {
                    while let Some(q) = self.peek(0) {
                        self.pos += 1;
                        if q == '\'' {
                            break;
                        }
                        value.push(q);
                    }
                }
                '"' => self.double_quoted(&mut value),
                '$' => self.dollar(&mut value),
                '`' => self.backtick(),
                '*' | '?' | '[' => {
                    self.out.plain = false;
                    value.push(c);
                }
                _ => value.push(c),
            }
        }
        Word {
            raw: self.chars[start..self.pos].iter().collect(),
            value,
        }
    }

    fn double_quoted(&mut self, value: &mut String) {
        while let Some(c) = self.peek(0) {
            self.pos += 1;
            match c {
                '"' => return,
                '\\' if matches!(self.peek(0), Some('$' | '`' | '"' | '\\' | '\n')) => {
                    value.push(self.chars[self.pos]);
                    self.pos += 1;
                }
                '$' => self.dollar(value),
                '`' => self.backtick(),
                _ => value.push(c),
            }
        }
    }

    /// After `$`: command substitution is parsed as nested commands; any expansion is not plain.
    fn dollar(&mut self, value: &mut String) {
        self.out.plain = false;
        value.push('$');
        if self.peek(0) != Some('(') {
            return;
        }
        let arithmetic = self.peek(1) == Some('(');
        let inner = self.balanced();
        if !arithmetic {
            self.nested(&inner);
        }
    }

    fn backtick(&mut self) {
        self.out.plain = false;
        let mut inner = String::new();
        while let Some(c) = self.peek(0) {
            self.pos += 1;
            match c {
                '`' => break,
                '\\' => {
                    if let Some(next) = self.peek(0) {
                        self.pos += 1;
                        inner.push(next);
                    }
                }
                _ => inner.push(c),
            }
        }
        self.nested(&inner);
    }

    /// Reads `( ... )` starting at `(`; returns the text between the outer parentheses.
    fn balanced(&mut self) -> String {
        let start = self.pos + 1;
        let mut depth = 0usize;
        let mut quote = None;
        while let Some(c) = self.peek(0) {
            self.pos += 1;
            match (quote, c) {
                (Some(q), _) if c == q => quote = None,
                (Some(_), '\\') | (None, '\\') => self.pos += 1,
                (Some(_), _) => {}
                (None, '\'' | '"') => quote = Some(c),
                (None, '(') => depth += 1,
                (None, ')') => {
                    depth -= 1;
                    if depth == 0 {
                        let end = self.pos - 1;
                        return self.chars[start.min(end)..end].iter().collect();
                    }
                }
                _ => {}
            }
        }
        self.pos = self.pos.min(self.chars.len());
        self.chars[start.min(self.pos)..].iter().collect()
    }

    fn nested(&mut self, inner: &str) {
        let script = parse(inner);
        self.out.commands.extend(script.commands);
    }
}

#[cfg(test)]
#[path = "shell_tests.rs"]
mod tests;
