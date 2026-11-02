//! A captcha service other apps of an organization verify against in the same request
//! (`env::xcall_call`): a signed-in member asks for a challenge, answers it in another
//! app's call, and that app calls `verify` here before it writes. A challenge is spent by
//! its first `verify`, right or wrong, and expires after `CHALLENGE_TTL_MS`. Method names
//! and arguments are the ABI's camelCase ones.
//!
//! The puzzle is a sum, to keep the example small; a real service would render an image
//! or ask a question from a set, with the answer kept here and never returned.

use calimero_app_sdk::env::Caller;
use calimero_app_sdk::prelude::*;

#[derive(Record)]
#[table(key = "id")]
pub struct Challenge {
    pub id: String,
    pub answer: String,
    pub expires_ms: u64,
    /// The app whose `verify` spent it; `None` while it is open.
    pub spent_by: Option<String>,
}

#[app::state(version = 1)]
pub struct Captcha {
    challenges: Table<Challenge>,
}

#[app::error]
pub enum Error {
    /// `verify` is for apps (synchronous xcalls), not for people.
    AppsOnly,
    LoginRequired,
}

/// What a client shows: never the answer.
#[derive(serde::Serialize)]
pub struct Puzzle {
    pub id: String,
    pub question: String,
}

/// How long a challenge may be answered.
pub const CHALLENGE_TTL_MS: u64 = 5 * 60 * 1000;

#[app::logic]
impl Captcha {
    #[app::init]
    pub fn init() -> app::Result<Self> {
        Ok(Self::load())
    }

    /// A new challenge for a signed-in caller.
    pub fn challenge(&mut self) -> app::Result<Puzzle> {
        if !matches!(env::caller(), Caller::Account(_) | Caller::ApiKey { .. }) {
            app::bail!(Error::LoginRequired);
        }
        let id = env::unique_id();
        let r = env::random_bytes(2);
        let (a, b) = (u32::from(r[0] % 50), u32::from(r[1] % 50));
        self.challenges.insert(Challenge {
            id: id.clone(),
            answer: (a + b).to_string(),
            expires_ms: env::time_now().saturating_add(CHALLENGE_TTL_MS),
            spent_by: None,
        })?;
        Ok(Puzzle {
            id,
            question: format!("What is {a} + {b}?"),
        })
    }

    /// Whether `answer` solves challenge `id`. Called by another app of the organization
    /// in its own call (`env::xcall_call`), which waits for the answer. The challenge is
    /// spent either way, so it cannot be guessed at, and an unknown, spent or expired one
    /// is `false`.
    #[app::allow(app = "*")]
    pub fn verify(&mut self, id: String, answer: String) -> app::Result<bool> {
        let Caller::App(app) = env::caller() else {
            app::bail!(Error::AppsOnly);
        };
        let Some(mut c) = self.challenges.get(&id)? else {
            return Ok(false);
        };
        if c.spent_by.is_some() || env::time_now() > c.expires_ms {
            return Ok(false);
        }
        let solved = c.answer == answer.trim();
        c.spent_by = Some(app);
        self.challenges.upsert(c)?;
        Ok(solved)
    }

    /// `"open"`, `"spent"`, `"expired"` or `"unknown"`, for any app of the organization
    /// (`env::xcall_view`) and for signed-in members.
    #[app::view]
    #[app::allow(app = "*")]
    pub fn status(&self, id: String) -> app::Result<String> {
        Ok(match self.challenges.get(&id)? {
            None => "unknown",
            Some(c) if c.spent_by.is_some() => "spent",
            Some(c) if env::time_now() > c.expires_ms => "expired",
            Some(_) => "open",
        }
        .to_owned())
    }

    /// The answer of challenge `id`, for the organization's owner: tests and support only.
    #[app::view]
    pub fn answer_of(&self, id: String) -> app::Result<Option<String>> {
        let owner = matches!(env::caller(), Caller::Account(a) if a == env::app_owner());
        if !owner {
            return Ok(None);
        }
        Ok(self.challenges.get(&id)?.map(|c| c.answer))
    }
}
