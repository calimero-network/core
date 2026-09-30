//! The two analyzers every index registers.
//!
//! - [`WORDS`]: UAX #29 word boundaries, NFKD with combining marks dropped
//!   (so `café` and `cafe` are one term), lowercased; runs of CJK ideographs,
//!   which UAX #29 splits one character per word, become overlapping bigrams.
//! - [`TRIGRAMS`]: the same fold over the whole text, whitespace collapsed,
//!   cut into overlapping 3-character windows at consecutive positions — so a
//!   phrase query over a query's trigrams is an exact substring match.
//!
//! Queries go through [`fold`] too, so both sides always agree. Changing any of
//! this changes the terms on disk: bump [`TOKENIZER_VERSION`], which makes
//! every node rebuild its indexes.

use tantivy::tokenizer::{TextAnalyzer, Token, TokenStream, Tokenizer};
use unicode_normalization::char::is_combining_mark;
use unicode_normalization::UnicodeNormalization;
use unicode_segmentation::UnicodeSegmentation;

/// Name the word analyzer is registered under.
pub const WORDS: &str = "cal_words";

/// Name the trigram analyzer is registered under.
pub const TRIGRAMS: &str = "cal_tri";

/// Version of everything in this module that shapes a term.
pub const TOKENIZER_VERSION: u32 = 1;

/// Words longer than this (in bytes, after folding) are not indexed.
const MAX_WORD_LEN: usize = 64;

/// NFKD, combining marks dropped, lowercased.
#[must_use]
pub fn fold(text: &str) -> String {
    text.nfkd()
        .filter(|c| !is_combining_mark(*c))
        .flat_map(char::to_lowercase)
        .collect()
}

fn is_cjk(c: char) -> bool {
    matches!(u32::from(c),
        0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0x20000..=0x2FA1F
        | 0x3040..=0x30FF | 0xAC00..=0xD7AF)
}

/// Split `text` into word tokens, folded.
#[must_use]
pub fn words(text: &str) -> Vec<Token> {
    let mut out = Vec::new();
    // A pending run of single-character CJK words: (offset_from, offset_to, char).
    let mut run: Vec<(usize, usize, char)> = Vec::new();

    fn flush_run(run: &mut Vec<(usize, usize, char)>, out: &mut Vec<Token>) {
        match run.len() {
            0 => {}
            1 => {
                let (from, to, c) = run[0];
                push(out, from, to, fold(&c.to_string()));
            }
            _ => {
                for pair in run.windows(2) {
                    let text: String = [pair[0].2, pair[1].2].iter().collect();
                    push(out, pair[0].0, pair[1].1, fold(&text));
                }
            }
        }
        run.clear();
    }

    fn push(out: &mut Vec<Token>, from: usize, to: usize, text: String) {
        if text.is_empty() || text.len() > MAX_WORD_LEN {
            return;
        }
        let position = out.len();
        out.push(Token {
            offset_from: from,
            offset_to: to,
            position,
            text,
            position_length: 1,
        });
    }

    for (offset, word) in text.unicode_word_indices() {
        let mut chars = word.chars();
        let single_cjk = match (chars.next(), chars.next()) {
            (Some(c), None) if is_cjk(c) => Some(c),
            _ => None,
        };
        match single_cjk {
            Some(c) => {
                if run.last().is_some_and(|last| last.1 != offset) {
                    flush_run(&mut run, &mut out);
                }
                run.push((offset, offset + word.len(), c));
            }
            None => {
                flush_run(&mut run, &mut out);
                push(&mut out, offset, offset + word.len(), fold(word));
            }
        }
    }
    flush_run(&mut run, &mut out);
    out
}

/// The folded, whitespace-collapsed form trigrams are cut from.
#[must_use]
pub fn trigram_text(text: &str) -> Vec<char> {
    let mut out = Vec::with_capacity(text.len());
    let mut space = true;
    for c in fold(text).chars() {
        if c.is_whitespace() {
            if !space {
                out.push(' ');
            }
            space = true;
        } else {
            out.push(c);
            space = false;
        }
    }
    if out.last() == Some(&' ') {
        let _ = out.pop();
    }
    out
}

/// The trigrams of `text`, at consecutive positions.
#[must_use]
pub fn trigrams(text: &str) -> Vec<Token> {
    let chars = trigram_text(text);
    chars
        .windows(3)
        .enumerate()
        .map(|(position, w)| Token {
            offset_from: position,
            offset_to: position + 3,
            position,
            text: w.iter().collect(),
            position_length: 1,
        })
        .collect()
}

/// A [`Tokenizer`] over a precomputed token list.
#[derive(Clone, Copy, Debug)]
pub struct Analyzer {
    split: fn(&str) -> Vec<Token>,
}

/// The token stream of [`Analyzer`].
#[derive(Debug)]
pub struct VecTokenStream {
    tokens: Vec<Token>,
    at: usize,
}

impl TokenStream for VecTokenStream {
    fn advance(&mut self) -> bool {
        if self.at < self.tokens.len() {
            self.at += 1;
            true
        } else {
            false
        }
    }

    fn token(&self) -> &Token {
        &self.tokens[self.at - 1]
    }

    fn token_mut(&mut self) -> &mut Token {
        &mut self.tokens[self.at - 1]
    }
}

impl Tokenizer for Analyzer {
    type TokenStream<'a> = VecTokenStream;

    fn token_stream<'a>(&'a mut self, text: &'a str) -> VecTokenStream {
        VecTokenStream {
            tokens: (self.split)(text),
            at: 0,
        }
    }
}

/// Register [`WORDS`] and [`TRIGRAMS`] on `index`.
pub fn register(index: &tantivy::Index) {
    let tokenizers = index.tokenizers();
    tokenizers.register(WORDS, TextAnalyzer::from(Analyzer { split: words }));
    tokenizers.register(TRIGRAMS, TextAnalyzer::from(Analyzer { split: trigrams }));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(tokens: &[Token]) -> Vec<&str> {
        tokens.iter().map(|t| t.text.as_str()).collect()
    }

    #[test]
    fn words_fold_accents_and_case() {
        assert_eq!(
            texts(&words("Café CRÈME, naïve résumé! ﬁne")),
            ["cafe", "creme", "naive", "resume", "fine"]
        );
    }

    #[test]
    fn cjk_runs_become_bigrams() {
        let tokens = words("我爱北京 hello 猫");
        assert_eq!(texts(&tokens), ["我爱", "爱北", "北京", "hello", "猫"]);
        assert!(tokens.iter().enumerate().all(|(i, t)| t.position == i));
    }

    #[test]
    fn trigrams_collapse_whitespace_and_are_consecutive() {
        assert_eq!(texts(&trigrams("Ab  Cd")), ["ab ", "b c", " cd"]);
        assert!(trigrams("ab").is_empty());
    }
}
