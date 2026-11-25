//! What a request writes to the node's log: merod logs at INFO, and every line
//! ships to the operator, so no line may carry the request's URL path, query,
//! body or a name the guest wrote (review finding F6).

use std::io;
use std::sync::{Arc, Mutex};

use calimero_primitives::outbound::{OutboundMethod, OutboundRequest};
use tracing_subscriber::fmt::MakeWriter;

use super::*;

const PREFIX: &str = "https://api.example.com/v1/";
/// What a user put in the path and query: it must never reach the log.
const USER: &str = "alice@example.com";
const TOKEN: &str = "tok-5ecret-in-a-query";

/// Answers every request with 200 and an empty body.
#[derive(Debug)]
struct Ok200;

impl Transport for Ok200 {
    fn send(&self, _request: &Prepared) -> EyreResult<(u16, Vec<u8>)> {
        Ok((200, Vec::new()))
    }
}

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl io::Write for Captured {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl MakeWriter<'_> for Captured {
    type Writer = Self;

    fn make_writer(&self) -> Self {
        self.clone()
    }
}

/// What `f` logs at INFO and above, merod's production level.
fn logged(f: impl FnOnce()) -> String {
    let out = Captured::default();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(out.clone())
        .with_ansi(false)
        .with_max_level(tracing::Level::INFO)
        .finish();
    tracing::subscriber::with_default(subscriber, f);
    let bytes = out.0.lock().unwrap().clone();
    String::from_utf8(bytes).unwrap()
}

fn app() -> AppIdentity {
    AppIdentity {
        package: "com.example.app".to_owned(),
        signer: "did:key:zApp".to_owned(),
        secrets: BTreeMap::new(),
        own: OwnSecrets::default(),
    }
}

/// Fails every request with the transport's own words, which quote the URL.
#[derive(Debug)]
struct Fails;

impl Transport for Fails {
    fn send(&self, request: &Prepared) -> EyreResult<(u16, Vec<u8>)> {
        Err(eyre::eyre!(
            "connection reset while sending {}",
            request.url
        ))
    }
}

fn run(approved: bool) -> Arc<dyn OutboundHost> {
    run_over(approved, Arc::new(Ok200))
}

fn run_over(approved: bool, transport: Arc<dyn Transport>) -> Arc<dyn OutboundHost> {
    let service = OutboundService::with_parts(&["api.example.com"], None, transport);
    let approval = Approval {
        prefixes: if approved {
            vec![PREFIX.to_owned()]
        } else {
            vec![]
        },
        reviewed: vec![PREFIX.to_owned()],
    };
    service
        .for_run(
            app(),
            vec![PREFIX.to_owned()],
            Some(approval),
            "ctx-1".to_owned(),
        )
        .expect("the app declares a URL")
}

fn get(url: &str, body: &str) -> Vec<u8> {
    borsh::to_vec(&OutboundRequest {
        method: if body.is_empty() {
            OutboundMethod::Get
        } else {
            OutboundMethod::Post
        },
        url: url.to_owned(),
        headers: vec![],
        body: body.as_bytes().to_vec(),
    })
    .unwrap()
}

fn assert_no_input(log: &str) {
    for input in [USER, TOKEN, "users/", "secret:"] {
        assert!(!log.contains(input), "the log carries {input:?}:\n{log}");
    }
}

/// Reproduction (F6): a refused request logged its refusal's words, which quote the
/// whole URL, query included; a refused secret name quoted what the guest wrote.
#[test]
fn a_refused_request_logs_its_kind_and_host_not_its_url() {
    let outside = format!("https://api.example.com/users/{USER}?token={TOKEN}");
    let mut refusal = String::new();
    let log = logged(|| {
        refusal = run(true).send(&get(&outside, "")).unwrap_err();
    });
    assert!(
        refusal.contains(USER),
        "the guest still reads why: {refusal}"
    );
    assert!(log.contains("outbound request refused"), "{log}");
    assert!(log.contains("refusal=\"Invalid\""), "{log}");
    assert!(log.contains("host=\"api.example.com\""), "{log}");
    assert_no_input(&log);

    let named = format!("{{{{secret:{USER}}}}}");
    let log = logged(|| {
        let _refused = run(true).send(&get(PREFIX, &named)).unwrap_err();
    });
    assert!(log.contains("refusal=\"Invalid\""), "{log}");
    assert_no_input(&log);
}

#[test]
fn a_refusal_an_app_acts_on_is_logged_by_its_word() {
    let log = logged(|| {
        let _refused = run(false)
            .send(&get(&format!("{PREFIX}users/{USER}"), ""))
            .unwrap_err();
    });
    assert!(log.contains("refusal=\"NotApproved\""), "{log}");
    assert_no_input(&log);
}

/// Reproduction (F6): every request logged its URL's path, which can name a user.
#[test]
fn a_sent_request_logs_its_declared_prefix_not_its_path() {
    let url = format!("{PREFIX}users/{USER}?token={TOKEN}");
    let log = logged(|| {
        let _answer = run(true).send(&get(&url, "")).unwrap();
    });
    assert!(log.contains("outbound request"), "{log}");
    assert!(log.contains(&format!("prefix=\"{PREFIX}\"")), "{log}");
    assert_no_input(&log);
}

/// Reproduction (F6): a request the transport failed logged its URL's path, which
/// can name a user; it logs the declared prefix, as a sent one does.
#[test]
fn a_failed_request_logs_its_declared_prefix_not_its_path() {
    let url = format!("{PREFIX}users/{USER}?token={TOKEN}");
    let mut failed = String::new();
    let log = logged(|| {
        failed = run_over(true, Arc::new(Fails))
            .send(&get(&url, ""))
            .unwrap_err();
    });
    assert!(log.contains("outbound request failed"), "{log}");
    assert!(log.contains(&format!("prefix=\"{PREFIX}\"")), "{log}");
    assert_no_input(&log);
    assert_no_input(&failed);
}
