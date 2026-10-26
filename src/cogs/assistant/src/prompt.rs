//! What goes to the language model and what comes back from it.
//!
//! The answer is spoken, so the system prompt asks for one or two short plain sentences, and
//! the reply is cleaned for a speech synthesizer: reasoning blocks, markdown and runs of
//! whitespace are removed and the length is capped at a sentence boundary.

pub const SYSTEM: &str = "You are a voice assistant running on a small embedded board. \
You hear a spoken command and your answer is read aloud. Answer in one or two short plain \
sentences. No lists, no markdown, no emoji. If you do not know, say so briefly.";

pub const FALLBACK: &str = "Sorry, I could not reach my language model.";

/// Past exchanges, newest last.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct History {
    turns: Vec<(String, String)>,
    keep: usize,
}

impl History {
    pub fn new(keep: usize) -> Self {
        Self {
            turns: Vec::new(),
            keep,
        }
    }

    pub fn push(&mut self, user: &str, assistant: &str) {
        self.turns.push((user.to_string(), assistant.to_string()));
        while self.turns.len() > self.keep {
            self.turns.remove(0);
        }
    }

    pub fn len(&self) -> usize {
        self.turns.len()
    }

    /// The prompt text for `command`. With no past turns it is the command alone, which a
    /// chat model answers best; with some, they come first as a short transcript.
    pub fn prompt(&self, command: &str) -> String {
        if self.turns.is_empty() {
            return command.to_string();
        }
        let mut s = String::from("Earlier in this conversation:\n");
        for (u, a) in &self.turns {
            s.push_str(&format!("Q: {u}\nA: {a}\n"));
        }
        s.push_str(&format!("\nNow answer this: {command}"));
        s
    }
}

/// Remove `<think>...</think>` blocks (an unclosed one removes the rest).
fn strip_think(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find("<think>") {
        out.push_str(&rest[..i]);
        match rest[i..].find("</think>") {
            Some(j) => rest = &rest[i + j + "</think>".len()..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// Clean a model reply for speech; `max_chars` caps it at a sentence end where one exists.
pub fn for_speech(reply: &str, max_chars: usize) -> String {
    let t = strip_think(reply);
    let t: String = t
        .chars()
        .filter(|c| !matches!(c, '*' | '#' | '`' | '_' | '|'))
        .collect();
    let t = t
        .trim_start_matches(|c: char| c.is_whitespace() || c == '-' || c == '>')
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let t = t
        .strip_prefix("Assistant:")
        .map(str::trim)
        .unwrap_or(&t)
        .to_string();
    if t.chars().count() <= max_chars {
        return t;
    }
    let cut: String = t.chars().take(max_chars).collect();
    match cut.rfind(['.', '!', '?']) {
        Some(i) if i > max_chars / 3 => cut[..=i].to_string(),
        _ => cut.trim_end().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_prompt_carries_the_remembered_turns_oldest_first() {
        let mut h = History::new(2);
        assert_eq!(
            h.prompt("what time is it"),
            "what time is it",
            "no past turns: the command alone"
        );
        h.push("hi", "Hello.");
        h.push("how are you", "Fine.");
        h.push("and now", "Still fine.");
        assert_eq!(h.len(), 2, "only the last two are kept");
        assert_eq!(h.prompt("bye"), "Earlier in this conversation:\nQ: how are you\nA: Fine.\nQ: and now\nA: Still fine.\n\nNow answer this: bye");
        assert_eq!(History::new(0).prompt("x"), "x");
    }

    #[test]
    fn replies_are_cleaned_for_speech() {
        assert_eq!(
            for_speech(
                "<think>\nlet me think\n</think>\n\nIt is **three** o'clock.",
                300
            ),
            "It is three o'clock."
        );
        assert_eq!(for_speech("<think>never closed", 300), "");
        assert_eq!(
            for_speech("# Title\n- one\n- two", 300),
            "Title - one - two"
        );
        assert_eq!(for_speech("Assistant: Sure thing.", 300), "Sure thing.");
        assert_eq!(
            for_speech("  spaced   out\n\ntext ", 300),
            "spaced out text"
        );
    }

    #[test]
    fn long_replies_are_cut_at_a_sentence() {
        let long = "First sentence here. Second sentence goes on and on and on. Third one never ends and keeps going";
        assert_eq!(
            for_speech(long, 60),
            "First sentence here. Second sentence goes on and on and on."
        );
        let one = "x".repeat(200);
        assert_eq!(for_speech(&one, 50).len(), 50, "no boundary: hard cut");
    }
}
