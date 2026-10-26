//! The wake phrase gate: which transcripts are commands.
//!
//! First stage of the brick spike's two-stage gate is the energy segmenter in `asr`; this is
//! the second, the transcript. A transcript that contains the wake phrase (each word within one
//! edit when it has five letters or more) is a command, and the command is what follows the
//! phrase. A transcript that is only the phrase arms the gate for `listen_s`: the next
//! transcript inside that window is the command, phrase or not. Everything else is ignored.

pub const DEFAULT_PHRASE: &str = "hey arduino";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// What to do: the text after the wake phrase (or the whole armed transcript).
    Command(String),
    /// Only the phrase was said; the next transcript is the command.
    BareWake,
    Ignored,
}

/// Lower-case words with punctuation stripped (apostrophes and digits kept).
pub fn words(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|w| {
            w.chars()
                .filter(|c| c.is_alphanumeric() || *c == '\'')
                .collect::<String>()
                .to_lowercase()
        })
        .filter(|w| !w.is_empty())
        .collect()
}

fn edit_distance(a: &str, b: &str) -> usize {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for i in 1..=a.len() {
        let mut cur = vec![i];
        for j in 1..=b.len() {
            cur.push(
                (prev[j] + 1)
                    .min(cur[j - 1] + 1)
                    .min(prev[j - 1] + (a[i - 1] != b[j - 1]) as usize),
            );
        }
        prev = cur;
    }
    prev[b.len()]
}

fn word_matches(heard: &str, want: &str) -> bool {
    heard == want || (want.chars().count() >= 5 && edit_distance(heard, want) <= 1)
}

pub struct Gate {
    phrase: Vec<String>,
    listen_ns: u64,
    armed_until_ns: u64,
}

impl Gate {
    pub fn new(phrase: &str, listen_s: u64) -> Self {
        let mut p = words(phrase);
        if p.is_empty() {
            p = words(DEFAULT_PHRASE);
        }
        Self {
            phrase: p,
            listen_ns: listen_s * 1_000_000_000,
            armed_until_ns: 0,
        }
    }

    pub fn armed(&self, now_ns: u64) -> bool {
        now_ns < self.armed_until_ns
    }

    /// Where the phrase starts and ends in `w`, if it is there.
    fn find(&self, w: &[String]) -> Option<(usize, usize)> {
        let n = self.phrase.len();
        (0..=w.len().checked_sub(n)?)
            .find(|&i| (0..n).all(|k| word_matches(&w[i + k], &self.phrase[k])))
            .map(|i| (i, i + n))
    }

    /// Judge one transcript; `start_ns`/`end_ns` are its capture times.
    pub fn on_transcript(&mut self, text: &str, start_ns: u64, end_ns: u64) -> Outcome {
        let w = words(text);
        if let Some((_, end)) = self.find(&w) {
            let rest = w[end..].join(" ");
            return if rest.is_empty() {
                self.armed_until_ns = end_ns + self.listen_ns;
                Outcome::BareWake
            } else {
                self.armed_until_ns = 0;
                Outcome::Command(rest)
            };
        }
        if self.armed(start_ns) && !w.is_empty() {
            self.armed_until_ns = 0;
            return Outcome::Command(w.join(" "));
        }
        Outcome::Ignored
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: u64 = 1_000_000_000;

    #[test]
    fn the_command_is_what_follows_the_phrase() {
        let mut g = Gate::new("hey arduino", 8);
        assert_eq!(
            g.on_transcript("Hey Arduino, turn on the kitchen light.", 0, S),
            Outcome::Command("turn on the kitchen light".into())
        );
        assert_eq!(
            g.on_transcript("what's the weather", 10 * S, 11 * S),
            Outcome::Ignored
        );
        assert_eq!(
            g.on_transcript("Well, hey Arduino what time is it", 20 * S, 21 * S),
            Outcome::Command("what time is it".into()),
            "mid-sentence"
        );
    }

    #[test]
    fn near_misses_of_long_words_count_but_short_ones_do_not() {
        let mut g = Gate::new("hey arduino", 8);
        assert_eq!(
            g.on_transcript("hey arduin stop", 0, S),
            Outcome::Command("stop".into()),
            "one edit in arduino"
        );
        assert_eq!(
            g.on_transcript("hey arduino2 stop", 0, S),
            Outcome::Command("stop".into())
        );
        assert_eq!(
            g.on_transcript("hay arduino stop", 0, S),
            Outcome::Ignored,
            "hay is not hey: 3-letter words match exactly"
        );
        assert_eq!(g.on_transcript("hey ardvark stop", 0, S), Outcome::Ignored);
    }

    #[test]
    fn a_bare_wake_arms_the_next_transcript_for_a_while() {
        let mut g = Gate::new("hey arduino", 8);
        assert_eq!(g.on_transcript("Hey Arduino.", 0, 2 * S), Outcome::BareWake);
        assert!(g.armed(5 * S) && !g.armed(11 * S));
        assert_eq!(
            g.on_transcript("play some music", 4 * S, 6 * S),
            Outcome::Command("play some music".into())
        );
        assert!(!g.armed(7 * S), "one command per arming");
        assert_eq!(
            g.on_transcript("play some music", 8 * S, 9 * S),
            Outcome::Ignored
        );
        assert_eq!(
            g.on_transcript("hey arduino", 20 * S, 21 * S),
            Outcome::BareWake
        );
        assert_eq!(
            g.on_transcript("too late", 40 * S, 41 * S),
            Outcome::Ignored,
            "the window passed"
        );
    }

    #[test]
    fn a_new_wake_phrase_and_odd_input_are_handled() {
        let mut g = Gate::new("computer", 8);
        assert_eq!(
            g.on_transcript("Computer lights off", 0, S),
            Outcome::Command("lights off".into())
        );
        let mut d = Gate::new("   ", 8);
        assert_eq!(
            d.on_transcript("hey arduino go", 0, S),
            Outcome::Command("go".into()),
            "an empty phrase falls back to the default"
        );
        assert_eq!(d.on_transcript("", 0, S), Outcome::Ignored);
        assert_eq!(d.on_transcript("...", 0, S), Outcome::Ignored);
    }

    #[test]
    fn words_strip_punctuation() {
        assert_eq!(
            words("Hey, Arduino! It's 3 o'clock."),
            ["hey", "arduino", "it's", "3", "o'clock"]
        );
        assert_eq!(edit_distance("kitten", "sitting"), 3);
    }
}
