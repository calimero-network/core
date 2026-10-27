//! Deterministic chat corpora: a Zipf vocabulary of pronounceable pseudo-words
//! (so prefixes, infixes and typos mean something), mixed with realistic
//! sentences (accents, CJK, punctuation), 50 Zipf-distributed senders, and a
//! few planted terms of known frequency.

use calimero_primitives::search::{
    SearchDoc, SearchFieldKind, SearchFieldSchema, SearchIndexSchema, SearchValue,
};

/// Words in the Zipf vocabulary.
pub const VOCAB: usize = 20_000;

/// Distinct senders.
pub const SENDERS: usize = 50;

const SYLLABLES: [&str; 16] = [
    "ka", "ri", "to", "me", "su", "lo", "na", "pe", "vi", "do", "ga", "hu", "ze", "bo", "fi", "ly",
];

const SENTENCES: [&str; 32] = [
    "the merger closes friday, legal needs the signed copy",
    "can we move the standup to 10:30 tomorrow?",
    "Let's meet at the café near the station",
    "I pushed the fix, CI is green now",
    "who has the keys for the storage room",
    "Reminder: quarterly review deck due Monday",
    "the naïve approach times out at 100k rows",
    "Résumé attached — thanks for the referral!",
    "lunch? thinking ramen or the new Thai place",
    "deploy is blocked on the database migration",
    "我们明天在北京开会",
    "東京の会議は延期になりました",
    "the invoice number is INV-20931, please check",
    "flight lands at 6pm, I'll take a taxi",
    "does anyone know how to reset the router",
    "great job on the launch everyone 🎉",
    "the client wants a discount on the renewal",
    "Please review PR #4180 before the release",
    "Straße closed due to construction, take the detour",
    "the backup job failed again last night",
    "shipping the new onboarding flow next sprint",
    "can someone cover my on-call shift saturday",
    "Coffee machine on floor 3 is broken again",
    "the contract says net-30 payment terms",
    "please rotate the API keys after the audit",
    "Happy birthday Zoë! 🎂",
    "our crème brûlée recipe needs more vanilla",
    "the latency spike was caused by a cold cache",
    "Moving the offsite to the lake house in June",
    "the roadmap draft is in the shared folder",
    "see you at the São Paulo office next week",
    "bug: search returns stale results after edit",
];

/// One message.
#[derive(Clone, Debug)]
pub struct Message {
    pub id: [u8; 32],
    pub sender: String,
    pub text: String,
    pub ts: u64,
}

/// Deterministic xorshift.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// A Zipf(s≈1) rank in `0..n`: inverse CDF of the continuous harmonic.
    pub fn zipf(&mut self, n: usize) -> usize {
        let u = (self.next() % 1_000_000) as f64 / 1_000_000.0;
        ((n as f64).powf(u) as usize).saturating_sub(1).min(n - 1)
    }

    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// The word of Zipf rank `i`: two to four syllables, unique per rank.
#[must_use]
pub fn word(i: usize) -> String {
    let mut out = String::new();
    let mut n = i;
    for k in 0..4 {
        if k >= 2 && n == 0 {
            break;
        }
        out.push_str(SYLLABLES[n % 16]);
        n /= 16;
    }
    out
}

/// The sender of rank `i`.
#[must_use]
pub fn sender(i: usize) -> String {
    format!("user{i}")
}

fn id(context_salt: u64, i: usize) -> [u8; 32] {
    let mut id = [0_u8; 32];
    id[..8].copy_from_slice(&context_salt.to_be_bytes());
    id[8..16].copy_from_slice(&(i as u64).to_be_bytes());
    id[31] = 0xAB;
    id
}

/// `n` messages. Every 200th contains `needle`; exactly five contain
/// `zebrafish`; ~30% are a realistic sentence with a few words mixed in.
#[must_use]
pub fn messages(n: usize, seed: u64) -> Vec<Message> {
    let mut r = Rng(0x9e37_79b9_7f4a_7c15 ^ seed.wrapping_mul(0x2545_F491_4F6C_DD1D));
    (0..n)
        .map(|i| {
            let mut words: Vec<String> = Vec::new();
            if r.below(10) < 3 {
                words.push(SENTENCES[r.below(SENTENCES.len())].to_owned());
                for _ in 0..3 {
                    words.push(word(r.zipf(VOCAB)));
                }
            } else {
                for _ in 0..(6 + r.below(12)) {
                    words.push(word(r.zipf(VOCAB)));
                }
            }
            if i % 200 == 7 {
                words.push("needle".to_owned());
            }
            if i % (n / 5).max(1) == 3 {
                words.push("zebrafish".to_owned());
            }
            Message {
                id: id(seed, i),
                sender: sender(r.zipf(SENDERS)),
                text: words.join(" "),
                ts: 1_700_000_000_000 + i as u64,
            }
        })
        .collect()
}

/// A "docs" corpus: `docs` documents of `blocks` paragraphs each, one search
/// document per block (the unit an editor re-indexes on a keystroke).
#[must_use]
pub fn blocks(docs: usize, blocks: usize, seed: u64) -> Vec<Message> {
    let mut r = Rng(0xdead_beef ^ seed);
    let mut out = Vec::with_capacity(docs * blocks);
    for d in 0..docs {
        for b in 0..blocks {
            let i = d * blocks + b;
            let mut words: Vec<String> = Vec::new();
            words.push(SENTENCES[r.below(SENTENCES.len())].to_owned());
            for _ in 0..(15 + r.below(30)) {
                words.push(word(r.zipf(VOCAB)));
            }
            if i % 200 == 7 {
                words.push("needle".to_owned());
            }
            out.push(Message {
                id: id(seed, i),
                sender: format!("doc{d}"),
                text: words.join(" "),
                ts: i as u64,
            });
        }
    }
    out
}

/// The index the chat app declares.
#[must_use]
pub fn schema() -> SearchIndexSchema {
    SearchIndexSchema {
        name: "messages".to_owned(),
        version: 1,
        fields: vec![
            SearchFieldSchema {
                name: "text".to_owned(),
                kind: SearchFieldKind::Text {
                    weight: 100,
                    infix: true,
                },
            },
            SearchFieldSchema {
                name: "sender".to_owned(),
                kind: SearchFieldKind::Keyword,
            },
            SearchFieldSchema {
                name: "ts".to_owned(),
                kind: SearchFieldKind::U64,
            },
        ],
    }
}

/// `m` as the app's extractor would hand it over.
#[must_use]
pub fn document(m: &Message) -> SearchDoc {
    SearchDoc {
        id: m.id,
        fields: vec![
            ("text".to_owned(), SearchValue::Str(m.text.clone())),
            ("sender".to_owned(), SearchValue::Str(m.sender.clone())),
            ("ts".to_owned(), SearchValue::U64(m.ts)),
        ],
    }
}
