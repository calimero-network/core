//! The `host-collections` feature (PoC): the same app built with and without it answers
//! the same results, cursors included, and leaves the same state, and either build reads
//! what the other wrote. `Table`, `UnorderedMap` and `Tally` go through
//! `calimero:app/collections` only in the feature build.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use calimero_app_build::{build_component_with_features, Built};
use calimero_sdk::testing::{Failure, Org, TestApp};
use serde_json::{json, Value};

const T0: u64 = 1_700_000_000_000;
const COLLECTIONS: &str = "calimero:app/collections";

/// The crate at `dir` (relative to this crate) as a component, feature on or off; each
/// build has its own target dir, so neither relinks the other.
fn build(dir: &str, on: bool) -> Built {
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join(dir);
    let target = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("host-collections")
        .join(if on { "on" } else { "off" });
    let features: &[&str] = if on { &["host-collections"] } else { &[] };
    build_component_with_features(&crate_dir, &target, features)
        .unwrap_or_else(|e| panic!("build {} (feature {on}): {e}", crate_dir.display()))
}

fn host_table(on: bool) -> &'static [u8] {
    static OFF: OnceLock<Built> = OnceLock::new();
    static ON: OnceLock<Built> = OnceLock::new();
    let cell = if on { &ON } else { &OFF };
    &cell
        .get_or_init(|| build("tests/guests/host_table", on))
        .component
}

fn kv_store(on: bool) -> &'static [u8] {
    static OFF: OnceLock<Built> = OnceLock::new();
    static ON: OnceLock<Built> = OnceLock::new();
    let cell = if on { &ON } else { &OFF };
    &cell
        .get_or_init(|| build("../../apps/kv-store", on))
        .component
}

fn imports_collections(component: &[u8]) -> bool {
    calimero_app_build::imports_of(component)
        .unwrap()
        .iter()
        .any(|i| i.starts_with(COLLECTIONS))
}

/// One step: its label, its answer and the fuel it took.
type Step = (String, Result<Value, Failure>, u64);

struct Run<'a> {
    t: &'a mut TestApp,
    steps: Vec<Step>,
}

impl Run<'_> {
    fn call(&mut self, method: &str, args: Value) -> Result<Value, Failure> {
        let res = self.t.as_("owner").call(method, args.clone());
        self.log(format!("call {method} {args}"), res)
    }

    fn view(&mut self, method: &str, args: Value) -> Result<Value, Failure> {
        let res = self.t.as_("owner").view(method, args.clone());
        self.log(format!("view {method} {args}"), res)
    }

    fn log(&mut self, label: String, res: Result<Value, Failure>) -> Result<Value, Failure> {
        let fuel = self.t.last_usage().fuel;
        self.steps.push((label, res.clone(), fuel));
        res
    }

    /// Every page of a view taking `cursor`, following `next` until there is none.
    fn pages(&mut self, method: &str, args: Value) {
        let mut args = args;
        for _ in 0..100 {
            let Ok(page) = self.view(method, args.clone()) else {
                return;
            };
            match page["next"].as_str() {
                Some(next) => args["cursor"] = json!(next),
                None => return,
            }
        }
        panic!("{method}: more than 100 pages");
    }
}

fn owner_of(id: u64) -> &'static str {
    ["ann", "bob_two", "cy"][(id % 3) as usize]
}

/// The writes of the scenario: every table, map and tally write and their refusals.
fn writes(r: &mut Run<'_>) {
    for id in 1..=25u64 {
        let email = (id % 4 != 0).then(|| format!("u{id}@x"));
        let _ = r.call(
            "insert",
            json!({"itemId": id.to_string(), "ownerName": owner_of(id), "email": email, "note": format!("n{id}")}),
        );
    }
    // A taken key, a unique clash, two records without an email.
    let _ = r.call(
        "insert",
        json!({"itemId": "3", "ownerName": "ann", "email": null, "note": "x"}),
    );
    let _ = r.call(
        "insert",
        json!({"itemId": "30", "ownerName": "ann", "email": "u1@x", "note": "x"}),
    );
    let _ = r.call(
        "insert",
        json!({"itemId": "31", "ownerName": "cy", "email": null, "note": "x"}),
    );
    // Upsert: replace (owner and index move), insert, clash.
    let _ = r.call(
        "upsert",
        json!({"itemId": "2", "ownerName": "ann", "email": "new2@x", "note": "up"}),
    );
    let _ = r.call(
        "upsert",
        json!({"itemId": "40", "ownerName": "dee", "email": null, "note": "new"}),
    );
    let _ = r.call(
        "upsert",
        json!({"itemId": "41", "ownerName": "dee", "email": "u5@x", "note": "x"}),
    );
    // Update: note, owner, email (a clash, then none), the key (refused), an absent id.
    let _ = r.call("setNote", json!({"itemId": "5", "note": "changed"}));
    let _ = r.call("setNote", json!({"itemId": "999", "note": "x"}));
    let _ = r.call("setOwner", json!({"itemId": "6", "ownerName": "bob_two"}));
    let _ = r.call("setEmail", json!({"itemId": "7", "email": "u1@x"}));
    let _ = r.call("setEmail", json!({"itemId": "7", "email": null}));
    let _ = r.call("setId", json!({"itemId": "8", "newId": "9"}));
    let _ = r.call("remove", json!({"itemId": "10"}));
    let _ = r.call("remove", json!({"itemId": "10"}));

    for n in 1..=12u64 {
        let _ = r.call(
            "noteSet",
            json!({"who": "ann", "n": n.to_string(), "text": format!("a{n}")}),
        );
    }
    for (who, n) in [("bob", 1u64), ("bob", 2), ("annex", 1), ("zed", u64::MAX)] {
        let _ = r.call(
            "noteSet",
            json!({"who": who, "n": n.to_string(), "text": "t"}),
        );
    }
    let _ = r.call("noteSet", json!({"who": "ann", "n": "3", "text": "again"}));
    let _ = r.call("noteRemove", json!({"who": "bob", "n": "2"}));
    let _ = r.call("noteRemove", json!({"who": "bob", "n": "2"}));

    for (choice, delta) in [("yes", 2), ("yes", 3), ("no", -1), ("no", -4)] {
        let _ = r.call(
            "vote",
            json!({"choice": choice, "delta": delta.to_string()}),
        );
    }
}

/// The reads of the scenario, every page followed through its cursors.
fn reads(r: &mut Run<'_>) {
    for id in ["2", "5", "6", "7", "8", "10", "999"] {
        let _ = r.view("get", json!({"itemId": id}));
        let _ = r.view("contains", json!({"itemId": id}));
    }
    let find = |owner: Option<&str>, limit: Option<u32>, reverse: bool| json!({"index": "byOwner", "ownerName": owner, "cursor": null, "limit": limit, "reverse": reverse});
    r.pages("find", find(Some("ann"), Some(4), false));
    r.pages("find", find(Some("bob_two"), Some(3), true));
    r.pages("find", find(None, Some(7), false));
    r.pages("find", find(None, None, true));
    let _ = r.view("find", find(Some("ann"), Some(0), false));
    let _ = r.view("find", json!({"index": "nope", "ownerName": null, "cursor": null, "limit": null, "reverse": false}));
    // A cursor from another index (the scan) is refused.
    let scan = r.view(
        "scan",
        json!({"cursor": null, "limit": 1, "reverse": false}),
    );
    let other = scan
        .ok()
        .and_then(|p| p["next"].as_str().map(str::to_owned));
    let _ = r.view("find", json!({"index": "byOwner", "ownerName": null, "cursor": other, "limit": null, "reverse": false}));
    let _ = r.view("find", json!({"index": "byOwner", "ownerName": null, "cursor": "%%", "limit": null, "reverse": false}));
    r.pages(
        "scan",
        json!({"cursor": null, "limit": 6, "reverse": false}),
    );
    r.pages("scan", json!({"cursor": null, "limit": 5, "reverse": true}));
    let _ = r.view("scanFrom", json!({"itemId": "20"}));
    for owner in [
        Some("ann"),
        Some("bob_two"),
        Some("dee"),
        Some("nobody"),
        None,
    ] {
        let _ = r.view("count", json!({"name": "perOwner", "ownerName": owner}));
    }
    let _ = r.view("count", json!({"name": "nope", "ownerName": null}));

    for (who, n) in [("ann", "3"), ("bob", "2"), ("zed", "18446744073709551615")] {
        let _ = r.view("noteGet", json!({"who": who, "n": n}));
        let _ = r.view("noteHas", json!({"who": who, "n": n}));
    }
    r.pages(
        "notes",
        json!({"who": "ann", "cursor": null, "limit": 5, "reverse": false}),
    );
    r.pages(
        "notes",
        json!({"who": null, "cursor": null, "limit": 4, "reverse": true}),
    );
    r.pages(
        "notes",
        json!({"who": "bob", "cursor": null, "limit": null, "reverse": false}),
    );
    let _ = r.view("notesStarting", json!({"start": "ann"}));
    let _ = r.view("notesEnds", json!({}));
    let _ = r.view("notesLen", json!({}));

    for choice in ["yes", "no", "maybe"] {
        let _ = r.view("votes", json!({"choice": choice}));
    }
}

fn steps(t: &mut TestApp, scenario: impl FnOnce(&mut Run<'_>)) -> Vec<Step> {
    let mut r = Run {
        t,
        steps: Vec::new(),
    };
    scenario(&mut r);
    r.steps
}

/// The two builds' steps agree on every answer; prints the fuel of each.
fn assert_same(title: &str, off: &[Step], on: &[Step]) {
    assert_eq!(off.len(), on.len(), "{title}: step counts");
    eprintln!("{title}: fuel per step, feature off -> on");
    for ((label, a, fa), (_, b, fb)) in off.iter().zip(on) {
        eprintln!("  {fa:>9} -> {fb:>9}  {label}");
        assert_eq!(a, b, "{title}: {label}");
    }
    let total = |s: &[Step]| s.iter().map(|(_, _, f)| f).sum::<u64>();
    eprintln!("  total {} -> {}", total(off), total(on));
}

#[test]
fn same_state_with_and_without_host_collections() {
    assert!(!imports_collections(host_table(false)));
    assert!(
        imports_collections(host_table(true)),
        "the feature build calls the host's collections"
    );

    let mut off = TestApp::new(host_table(false), Org::new(), T0);
    let mut on = TestApp::new(host_table(true), Org::new(), T0);
    let scenario = |r: &mut Run<'_>| {
        writes(r);
        reads(r);
        // The guest-path clear, then the host reads what it left.
        let _ = r.call("notesClear", json!({}));
        let _ = r.view("notesLen", json!({}));
        r.pages(
            "notes",
            json!({"who": null, "cursor": null, "limit": 3, "reverse": false}),
        );
    };
    let a = steps(&mut off, scenario);
    let b = steps(&mut on, scenario);
    assert_same("host_table", &a, &b);
    // Sanity: the scenario does reach the refusals it means to compare.
    let answer = |label: &str| {
        a.iter()
            .find(|(l, _, _)| l.contains(label))
            .map(|(_, r, _)| r.clone())
            .unwrap()
    };
    assert!(
        matches!(answer("\"itemId\":\"30\""), Err(Failure::App { code, fields }) if code == "Duplicate" && fields["index"] == "byEmail")
    );
    assert!(
        matches!(answer("\"itemId\":\"3\",\"note\":\"x\""), Err(Failure::App { code, fields }) if code == "Duplicate" && fields["index"] == "key")
    );
    assert_eq!(off.export_state(), on.export_state(), "the same state");

    let mut off = TestApp::new(kv_store(false), Org::new(), T0);
    let mut on = TestApp::new(kv_store(true), Org::new(), T0);
    let kv = |r: &mut Run<'_>| {
        for (k, v) in [("b", "1"), ("a", "2"), ("c", "3"), ("a", "4")] {
            let _ = r.call("set", json!({"key": k, "value": v}));
        }
        let _ = r.call("updateIfExists", json!({"key": "b", "value": "5"}));
        let _ = r.call("updateIfExists", json!({"key": "z", "value": "5"}));
        let _ = r.call("getOrInsert", json!({"key": "c", "value": "6"}));
        let _ = r.call("getOrInsert", json!({"key": "d", "value": "7"}));
        let _ = r.view("entries", json!({}));
        let _ = r.view("len", json!({}));
        let _ = r.view("get", json!({"key": "a"}));
        let _ = r.view("getResult", json!({"key": "nope"}));
        let _ = r.call("remove", json!({"key": "a"}));
        let _ = r.call("remove", json!({"key": "a"}));
        let _ = r.view("entries", json!({}));
        let _ = r.call("clear", json!({}));
        let _ = r.view("len", json!({}));
        let _ = r.call("set", json!({"key": "e", "value": "8"}));
    };
    assert!(imports_collections(kv_store(true)));
    let a = steps(&mut off, kv);
    let b = steps(&mut on, kv);
    assert_same("kv-store", &a, &b);
    assert_eq!(off.export_state(), on.export_state(), "the same state");
}

#[test]
fn guest_written_state_reads_back_through_host_collections() {
    assert!(imports_collections(host_table(true)));
    // Written by the guest path, read by the guest path...
    let mut off = TestApp::new(host_table(false), Org::new(), T0);
    let _ = steps(&mut off, writes);
    let first = off
        .as_("owner")
        .view(
            "notes",
            json!({"who": "ann", "cursor": null, "limit": 5, "reverse": false}),
        )
        .unwrap();
    let cursor = first["next"].as_str().unwrap().to_owned();
    let second = off
        .as_("owner")
        .view(
            "notes",
            json!({"who": "ann", "cursor": cursor, "limit": 5, "reverse": false}),
        )
        .unwrap();
    let want = steps(&mut off, reads);
    let state = off.export_state();

    // ...then the same state read by the feature build.
    off.replace_component(host_table(true));
    let on = &mut off;
    let got = steps(on, reads);
    assert_same("read back", &want, &got);
    // A cursor the guest path handed out continues on the host.
    let cont = on
        .as_("owner")
        .view(
            "notes",
            json!({"who": "ann", "cursor": cursor, "limit": 5, "reverse": false}),
        )
        .unwrap();
    assert_eq!(cont, second);
    assert_eq!(on.export_state(), state, "reads wrote nothing");

    // And the host's writes on guest-written state land where the guest path puts them.
    let mut guest = TestApp::new(host_table(false), Org::new(), T0);
    let _ = steps(&mut guest, writes);
    let more = |r: &mut Run<'_>| {
        let _ = r.call(
            "upsert",
            json!({"itemId": "3", "ownerName": "dee", "email": "u3@x", "note": "moved"}),
        );
        let _ = r.call("remove", json!({"itemId": "4"}));
        let _ = r.call(
            "insert",
            json!({"itemId": "50", "ownerName": "dee", "email": "u1@x", "note": "clash"}),
        );
        let _ = r.call("noteSet", json!({"who": "ann", "n": "1", "text": "z"}));
        let _ = r.call("noteRemove", json!({"who": "ann", "n": "2"}));
        let _ = r.call("vote", json!({"choice": "no", "delta": "5"}));
        reads(r);
    };
    let want = steps(&mut guest, more);
    on.replace_component(host_table(true));
    let got = steps(on, more);
    assert_same("host writes on guest state", &want, &got);
    assert_eq!(on.export_state(), guest.export_state());
}
