//! Rooms (`apps/rooms`) on the in-process three-node cluster: acceptance criteria A1
//! (the shared scenario), A3 (no double booking under contention) and A5 (followers honour
//! `minIndex` and hide titles), spec `docs/specs/app-runtime-v0.14.md` section 12.
//! A4 (the fault test) runs against real processes: `tests/fault/rooms_fault.py`.

mod common;

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use calimero_client::{Client, Consistency, Error as ClientError};
use calimero_identity::testing::SoftAuthenticator;
use calimero_node::app::Status as AppStatus;
use calimero_node::node::due_slot;
use common::{Cluster, ORIGIN};
use futures_util::StreamExt;
use serde_json::{json, Value};

/// The scenario's time zero: a slot boundary (a multiple of 15 minutes), the same value
/// `apps/rooms/tests/scenarios.rs` gives the harness.
const T0: u64 = 1_700_000_100_000;
const SLOT_MS: u64 = 900_000;

/// A three-node cluster whose app clock starts at `T0`, so the scenario's absolute times
/// are neither in the past nor the future of the app.
async fn cluster_at_t0() -> Cluster {
    let now = i64::try_from(calimero_node::types::now_ms()).unwrap();
    let offset = i64::try_from(T0).unwrap() - now;
    Cluster::start_with(3, |c| c.clock_offset_ms = offset).await
}

/// A person of the org: their account id, passkey and one signed-in client per node.
struct Person {
    id: String,
    pk: SoftAuthenticator,
    on: Vec<Client>,
}

/// An org with Rooms deployed in it by `owner`, who also created the org.
struct Org {
    id: String,
    app: String,
    people: BTreeMap<String, Person>,
    /// A client with no session, per node.
    anonymous: Vec<Client>,
}

impl Org {
    /// Signs up `owner` and each of `roles` (`(name, "admin" | "member" | "guest")`), signs
    /// each in on every node, and deploys Rooms in the owner's org.
    async fn setup(c: &Cluster, roles: &[(&str, &str)]) -> Self {
        let mut owner = person(c, "owner").await;
        let org = str_of(
            &owner.on[0]
                .post("orgs", json!({ "name": "Acme" }))
                .await
                .unwrap(),
            "orgId",
        );
        let mut people = BTreeMap::new();
        for (name, role) in roles {
            let p = person(c, name).await;
            let token = str_of(
                &owner.on[0]
                    .post_confirmed(
                        &format!("orgs/{org}/invites"),
                        json!({ "role": role }),
                        &mut owner.pk,
                    )
                    .await
                    .unwrap(),
                "token",
            );
            let _r = p.on[0]
                .post(&format!("invites/{token}/redeem"), json!({}))
                .await
                .unwrap();
            let _prev = people.insert((*name).to_owned(), p);
        }
        let bundle = owner.on[0].upload_bundle(common::rooms()).await.unwrap();
        let app = owner.on[0]
            .create_app("rooms", &bundle, json!({ "org": org }))
            .await
            .unwrap();
        let _prev = people.insert("owner".to_owned(), owner);
        Self {
            id: org,
            app,
            people,
            anonymous: (0..3).map(|i| c.client(i)).collect(),
        }
    }

    fn on(&self, name: &str, node: usize) -> &Client {
        &self.people[name].on[node]
    }
}

/// Signs `name` up on node 0 and in on the other two.
async fn person(c: &Cluster, name: &str) -> Person {
    let (first, mut pk) = common::signed_up(c, 0, name).await;
    let id = str_of(&first.me().await.unwrap(), "accountId");
    let mut on = vec![first];
    for i in 1..3 {
        let client = c.client(i);
        let _s = client.sign_in(&mut pk).await.unwrap();
        on.push(client);
    }
    Person { id, pk, on }
}

fn str_of(v: &Value, k: &str) -> String {
    v[k].as_str()
        .unwrap_or_else(|| panic!("{k} in {v}"))
        .to_owned()
}

// ---- The scenario runner: the step format of `calimero_app_sdk::testing::scenario`,
// against the cluster. Step `i` goes to node `i % 3`.

/// How a step ended, in the harness's terms (`calimero_app_sdk::testing::Failure`).
enum Failure {
    App {
        code: String,
        fields: Value,
    },
    Trap(String),
    ConfirmRequired,
    /// The platform refused before the app ran (Ruling 32): the HTTP status and API code.
    Refused {
        status: u16,
        code: String,
    },
    /// No answer from the API (transport).
    Other(String),
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::App { code, fields } => write!(f, "app error {code} {fields}"),
            Self::Trap(kind) => write!(f, "trap {kind}"),
            Self::ConfirmRequired => write!(f, "confirm required"),
            Self::Refused { status, code } => write!(f, "refused {status} {code}"),
            Self::Other(e) => write!(f, "{e}"),
        }
    }
}

/// A step's answer, for a message.
fn show(r: &Result<Value, Failure>) -> String {
    match r {
        Ok(v) => format!("ok {v}"),
        Err(f) => f.to_string(),
    }
}

/// The node's answer in the harness's terms, as it is: the app's error, a trap, a confirm
/// challenge, or any other API refusal with its status and code.
fn failure(e: &ClientError) -> Failure {
    let details = e.details().cloned().unwrap_or(Value::Null);
    match e {
        ClientError::Api { code, .. } if code == "APP_ERROR" => Failure::App {
            code: details["code"].as_str().unwrap_or_default().to_owned(),
            fields: details["fields"].clone(),
        },
        ClientError::Api { code, .. } if code == "TRAP" => {
            Failure::Trap(details["kind"].as_str().unwrap_or_default().to_owned())
        }
        ClientError::Api { code, .. } if code == "CONFIRM_REQUIRED" => Failure::ConfirmRequired,
        ClientError::Api { status, code, .. } => Failure::Refused {
            status: *status,
            code: code.clone(),
        },
        ClientError::WrongNode { .. } => Failure::Refused {
            status: 421,
            code: "WRONG_NODE".into(),
        },
        ClientError::Transport(e) => Failure::Other(e.to_string()),
    }
}

/// Every event each account received over its stream, in order: `(index, name)`.
type Received = Arc<Mutex<Vec<(u64, String)>>>;

struct Runner<'a> {
    c: &'a Cluster,
    org: &'a mut Org,
    vars: HashMap<String, Value>,
    received: BTreeMap<String, Received>,
    /// The node each account's event stream is opened on.
    home: BTreeMap<String, usize>,
    advanced: bool,
}

impl<'a> Runner<'a> {
    /// Opens one live event stream per account (on a node of its own), from the log's start.
    async fn new(c: &'a Cluster, org: &'a mut Org) -> Self {
        let mut received = BTreeMap::new();
        let mut home = BTreeMap::new();
        for (n, (name, p)) in org.people.iter().enumerate() {
            let node = n % 3;
            let got: Received = Arc::default();
            // A stream the node refuses receives nothing.
            if let Ok(mut stream) = p.on[node].events(&org.app, 0).await {
                let sink = Arc::clone(&got);
                drop(tokio::spawn(async move {
                    while let Some((id, kind, _data)) = stream.next().await {
                        let index = id.split(':').next().and_then(|i| i.parse().ok());
                        sink.lock().unwrap().push((index.unwrap_or(0), kind));
                    }
                }));
            }
            let _prev = received.insert(name.clone(), got);
            let _prev = home.insert(name.clone(), node);
        }
        Self {
            c,
            org,
            vars: HashMap::new(),
            received,
            home,
            advanced: false,
        }
    }

    async fn run(&mut self, path: &Path) {
        let text = std::fs::read_to_string(path).unwrap();
        let steps: Vec<Value> = serde_json::from_str(&text).unwrap();
        for (n, step) in steps.iter().enumerate() {
            if let Err(e) = self.step(n, step).await {
                panic!("step {n}: {step}\n{e}");
            }
        }
    }

    async fn step(&mut self, n: usize, step: &Value) -> Result<(), String> {
        let node = n % 3;
        let op = step["op"].as_str().ok_or("step needs an op")?;
        if step.get("save").is_some() && !matches!(op, "call" | "view") {
            return Err(format!("`save` only applies to a call or view, not {op:?}"));
        }
        let args = self.substitute(step.get("args").unwrap_or(&Value::Null))?;
        let result = match op {
            "call" | "view" => self.invoke(step, node, op, args).await?,
            "tick" => {
                if step.get("as").is_some() {
                    return Err("a tick always runs as the system; drop `as`".into());
                }
                self.tick(step["method"].as_str().ok_or("tick needs a method")?)
                    .await?
            }
            "advance" => {
                let ms = args.as_u64().ok_or("advance needs ms in args")?;
                self.c.advance_all(i64::try_from(ms).unwrap());
                self.advanced = true;
                Ok(Value::Null)
            }
            "removeMember" => {
                self.remove_member(args.as_str().ok_or("removeMember needs an account")?)
                    .await?;
                Ok(Value::Null)
            }
            other => return Err(format!("unknown op {other:?}")),
        };
        let expect = step.get("expect").map(|e| self.substitute(e)).transpose()?;
        check_expect(expect.as_ref(), &result)?;
        if let (Some(name), Ok(saved)) = (step.get("save").and_then(Value::as_str), &result) {
            let _prev = self.vars.insert(name.to_owned(), saved.clone());
        }
        if let Some(Value::Object(per_account)) = step.get("events") {
            for (account, names) in per_account {
                let want: Vec<String> = names
                    .as_array()
                    .ok_or_else(|| format!("events.{account} is not a list"))?
                    .iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect();
                self.events_reach(account, &want).await?;
            }
        }
        Ok(())
    }

    async fn invoke(
        &mut self,
        step: &Value,
        node: usize,
        op: &str,
        args: Value,
    ) -> Result<Result<Value, Failure>, String> {
        let method = step["method"].as_str().ok_or("step needs a method")?;
        let who = step["as"].as_str().unwrap_or("owner");
        let confirmed = step["confirmed"].as_bool() == Some(true);
        let app = self.org.app.clone();
        let answer = match who {
            "@system" => {
                return Err(
                    "`@system` calls are the harness's; no client runs as the system".into(),
                )
            }
            "@anonymous" => {
                let cl = &self.org.anonymous[node];
                if op == "call" {
                    cl.call(&app, method, args, None).await.map(|r| r.result)
                } else {
                    cl.view(&app, method, args, Consistency::Strong)
                        .await
                        .map(|r| r.result)
                }
            }
            account => {
                let p = self
                    .org
                    .people
                    .get_mut(account)
                    .ok_or_else(|| format!("no account {account}"))?;
                let cl = &p.on[node];
                match (op, confirmed) {
                    ("call", true) => cl
                        .call_confirmed(&app, method, args, &mut p.pk)
                        .await
                        .map(|r| r.result),
                    ("call", false) => cl.call(&app, method, args, None).await.map(|r| r.result),
                    _ => cl
                        .view(&app, method, args, Consistency::Strong)
                        .await
                        .map(|r| r.result),
                }
            }
        };
        Ok(answer.map_err(|e| failure(&e)))
    }

    /// Waits until the timer's latest due slot is applied on every node (the leader's
    /// 100 ms poll proposes it once app time passes it), and answers with the logged tick's
    /// outcome for that slot.
    async fn tick(&self, timer: &str) -> Result<Result<Value, Failure>, String> {
        let node = &self.c.nodes[0].as_ref().unwrap().node;
        let g = node.hosted(&self.org.app).ok_or("app not hosted")?;
        let (bundle, install) =
            g.read_local(|m, _| (m.state.bundle.clone(), m.state.component.head.install_ts_ms));
        let prepared = node
            .prepared_for(&bundle.ok_or("no bundle")?)
            .await
            .map_err(|e| format!("{e:?}"))?;
        let every = prepared
            .abi
            .methods
            .get(timer)
            .and_then(|m| m.every)
            .ok_or_else(|| format!("{timer} is not a timer"))?;
        let slot = due_slot(install, every, node.app_clock().now_ms())
            .ok_or_else(|| format!("no slot of {timer} is due"))?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let applied = self.c.nodes.iter().flatten().all(|r| {
                r.node
                    .hosted(&self.org.app)
                    .is_some_and(|g| g.read_local(|m, _| m.tick_applied(timer, slot)))
            });
            if applied {
                let logged = g.read_local(|m, _| {
                    m.ticks
                        .iter()
                        .find(|t| t.timer == timer && t.scheduled_ms == slot)
                        .map(|t| t.applied.clone())
                });
                let a = logged.ok_or_else(|| format!("no logged tick for slot {slot}"))?;
                return Ok(match a.status {
                    AppStatus::Ok => Ok(a.result),
                    AppStatus::AppError => Err(Failure::App {
                        code: a.error["code"].as_str().unwrap_or_default().to_owned(),
                        fields: a.error["fields"].clone(),
                    }),
                    AppStatus::Trap => Err(Failure::Trap(
                        a.error["kind"].as_str().unwrap_or_default().to_owned(),
                    )),
                });
            }
            if Instant::now() > deadline {
                return Err(format!("slot {slot} of {timer} never applied everywhere"));
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Removes `name` from the org once every stream has delivered what the account may see
    /// now (the node checks an event against membership when it sends it), then waits until
    /// every node applied the removal.
    async fn remove_member(&mut self, name: &str) -> Result<(), String> {
        self.flush_events().await?;
        let id = self.org.people[name].id.clone();
        let path = format!("orgs/{}/members/{id}", self.org.id);
        let owner = self
            .org
            .people
            .get_mut("owner")
            .ok_or("the org has no owner")?;
        let _r = owner.on[0]
            .delete_confirmed(&path, &mut owner.pk)
            .await
            .map_err(|e| e.to_string())?;
        for r in self.c.nodes.iter().flatten() {
            let applied = r.node.system.status().last_log.unwrap_or(0);
            if !r
                .node
                .system
                .wait_applied(applied, Duration::from_secs(5))
                .await
            {
                return Err("a node did not apply the removal".into());
            }
        }
        Ok(())
    }

    /// Waits until each live stream holds as many events as a fresh replay on its node.
    async fn flush_events(&self) -> Result<(), String> {
        let last = self.c.applied_on(0, &self.org.app);
        for (name, got) in &self.received {
            let node = self.home[name];
            let deadline = Instant::now() + Duration::from_secs(5);
            while self.c.applied_on(node, &self.org.app) < last {
                if Instant::now() > deadline {
                    return Err(format!("node {node} never applied {last}"));
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            let replayed = self.replay_count(name, node).await;
            while got.lock().unwrap().len() < replayed {
                if Instant::now() > deadline {
                    return Err(format!("{name}'s stream never caught up"));
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
        Ok(())
    }

    /// How many events a new stream for `name` on `node` replays (it sends the backlog at once).
    async fn replay_count(&self, name: &str, node: usize) -> usize {
        let Ok(mut stream) = self.org.on(name, node).events(&self.org.app, 0).await else {
            return 0;
        };
        let mut n = 0;
        while let Ok(Some(_)) =
            tokio::time::timeout(Duration::from_millis(300), stream.next()).await
        {
            n += 1;
        }
        n
    }

    /// Waits (up to 5 s) until `account` received exactly `want`, in order.
    async fn events_reach(&self, account: &str, want: &[String]) -> Result<(), String> {
        let got = self
            .received
            .get(account)
            .ok_or_else(|| format!("no account {account}"))?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let names: Vec<String> = got.lock().unwrap().iter().map(|(_, n)| n.clone()).collect();
            if names == want {
                return Ok(());
            }
            if Instant::now() > deadline {
                return Err(format!(
                    "events for {account}: expected {want:?}, got {names:?}"
                ));
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Replaces every string that is exactly `$name` by the saved value of `name`, and every
    /// string that is exactly `@name` by the account id of `name` (the harness: its name).
    fn substitute(&self, value: &Value) -> Result<Value, String> {
        Ok(match value {
            Value::String(s) => {
                if let Some(name) = s.strip_prefix('$').filter(|n| is_var_name(n)) {
                    self.vars
                        .get(name)
                        .cloned()
                        .ok_or_else(|| format!("unknown variable ${name}"))?
                } else if let Some(account) = s.strip_prefix('@').filter(|n| is_var_name(n)) {
                    let p = self
                        .org
                        .people
                        .get(account)
                        .ok_or_else(|| format!("unknown account @{account}"))?;
                    Value::String(p.id.clone())
                } else {
                    value.clone()
                }
            }
            Value::Array(items) => Value::Array(
                items
                    .iter()
                    .map(|v| self.substitute(v))
                    .collect::<Result<_, _>>()?,
            ),
            Value::Object(map) => Value::Object(
                map.iter()
                    .map(|(k, v)| Ok((k.clone(), self.substitute(v)?)))
                    .collect::<Result<_, String>>()?,
            ),
            other => other.clone(),
        })
    }
}

/// `[A-Za-z_][A-Za-z0-9_]*`.
fn is_var_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn check_expect(expect: Option<&Value>, result: &Result<Value, Failure>) -> Result<(), String> {
    let Some(expect) = expect else {
        return match result {
            Ok(_) => Ok(()),
            Err(f) => Err(format!("expected success, got {f}")),
        };
    };
    if let Some(want) = expect.get("ok") {
        return match result {
            Ok(got) if got == want => Ok(()),
            other => Err(format!("expected ok {want}, got {}", show(other))),
        };
    }
    if expect.get("confirmRequired").and_then(Value::as_bool) == Some(true) {
        return match result {
            Err(Failure::ConfirmRequired) => Ok(()),
            other => Err(format!("expected confirmRequired, got {}", show(other))),
        };
    }
    if let Some(code) = expect.get("error").and_then(Value::as_str) {
        return match result {
            Err(Failure::App { code: got, fields }) if got == code => match expect.get("fields") {
                Some(want) if want != fields => Err(format!(
                    "expected error {code} with fields {want}, got {fields}"
                )),
                _ => Ok(()),
            },
            other => Err(format!("expected error {code}, got {}", show(other))),
        };
    }
    if let Some(code) = expect.get("refused").and_then(Value::as_str) {
        return match result {
            Err(Failure::Refused { code: got, .. }) if got == code => Ok(()),
            other => Err(format!("expected refused {code}, got {}", show(other))),
        };
    }
    if let Some(kind) = expect.get("trap").and_then(Value::as_str) {
        return match result {
            Err(Failure::Trap(got)) if got == kind => Ok(()),
            other => Err(format!("expected trap {kind}, got {}", show(other))),
        };
    }
    Err("expect needs ok, error, trap, refused or confirmRequired".into())
}

const SCENARIO_ROLES: [(&str, &str); 4] = [
    ("ada", "admin"),
    ("bob", "member"),
    ("cy", "member"),
    ("gus", "guest"),
];

/// A1: the shared scenario passes on the cluster, its 16 minutes of app time in seconds,
/// and sessions opened before the clock moved still work after it.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn rooms_scenario_on_the_cluster() {
    let _built = common::rooms();
    let started = Instant::now();
    let c = cluster_at_t0().await;
    let mut org = Org::setup(&c, &SCENARIO_ROLES).await;
    let clock = c.nodes[0].as_ref().unwrap().node.app_clock().now_ms();
    assert!(
        (T0..T0 + 30_000).contains(&clock),
        "app clock {clock}, T0 {T0}"
    );

    let scenario =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../apps/rooms/scenarios/rooms.json");
    let mut runner = Runner::new(&c, &mut org).await;
    runner.run(&scenario).await;
    assert!(runner.advanced, "the scenario moves the clock");

    // Every session was minted before the advance; each still reaches the app on every node.
    for (name, p) in &org.people {
        for (i, cl) in p.on.iter().enumerate() {
            match cl
                .view(&org.app, "myBookings", json!({}), Consistency::Strong)
                .await
            {
                Ok(_) => {}
                Err(e) => {
                    let code = e.code();
                    assert!(
                        matches!(code, Some("APP_ERROR" | "NOT_FOUND")),
                        "{name} on node {i}: {e}"
                    );
                }
            }
            let me = cl
                .me()
                .await
                .unwrap_or_else(|e| panic!("{name} on node {i}: {e}"));
            assert_eq!(str_of(&me, "accountId"), p.id);
        }
    }
    let took = started.elapsed();
    assert!(took < Duration::from_secs(60), "took {took:?}");
    println!("A1 on the cluster: {took:?}");
}

/// The id of a booking `book` answered with.
fn booking_id(v: &Value) -> String {
    v.as_str()
        .unwrap_or_else(|| panic!("booking id {v}"))
        .to_owned()
}

/// A3: 200 accounts on three nodes book the same slot at once; one wins, 199 get `Taken`,
/// and every node's replica holds one booking for the slot.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn no_double_booking_under_contention() {
    const N: usize = 200;
    let c = Cluster::start(3).await;
    let mut owner = person(&c, "owner").await;
    let org = str_of(
        &owner.on[0]
            .post("orgs", json!({ "name": "Acme" }))
            .await
            .unwrap(),
        "orgId",
    );
    let bundle = owner.on[0].upload_bundle(common::rooms()).await.unwrap();
    let app = owner.on[0]
        .create_app("rooms", &bundle, json!({ "org": org }))
        .await
        .unwrap();
    let _r = owner.on[0]
        .call(
            &app,
            "addRoom",
            json!({ "id": "r1", "name": "Atlas", "capacity": 8 }),
            None,
        )
        .await
        .unwrap();
    let token = str_of(
        &owner.on[0]
            .post_confirmed(
                &format!("orgs/{org}/invites"),
                json!({ "role": "member", "uses": N }),
                &mut owner.pk,
            )
            .await
            .unwrap(),
        "token",
    );

    // 200 members, account k signed up on node k % 3.
    let mut members = Vec::new();
    for chunk in (0..N).collect::<Vec<_>>().chunks(25) {
        let joined = futures_util::future::join_all(chunk.iter().map(|k| {
            let (c, token) = (&c, &token);
            async move {
                let cl = c.client(k % 3);
                let mut pk = SoftAuthenticator::new("localhost", ORIGIN);
                let _s = cl.sign_up(&mut pk, &format!("m{k}")).await.unwrap();
                let _r = cl
                    .post(&format!("invites/{token}/redeem"), json!({}))
                    .await
                    .unwrap();
                cl
            }
        }))
        .await;
        members.extend(joined);
    }

    // A slot well after now, so no timer releases it during the test.
    let now = calimero_node::types::now_ms();
    let start = (now / SLOT_MS + 100) * SLOT_MS;
    let args = json!({ "roomId": "r1", "start": start.to_string(), "slotCount": 1, "title": "x" });
    let outcomes = futures_util::future::join_all(members.iter().enumerate().map(|(k, cl)| {
        let (app, args) = (&app, &args);
        async move {
            let key = format!("a3-{k}");
            // An unknown outcome (a 503) is retried with the same key.
            for _ in 0..20 {
                match cl.call(app, "book", args.clone(), Some(&key)).await {
                    Err(e) if e.code() == Some("TIMEOUT") || e.code() == Some("UNAVAILABLE") => {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                    other => return other,
                }
            }
            panic!("member {k}: no answer after 20 tries");
        }
    }))
    .await;

    let mut won = Vec::new();
    let mut taken = 0;
    for (k, o) in outcomes.into_iter().enumerate() {
        match o {
            Ok(r) => won.push(r),
            Err(e) => {
                assert_eq!(e.code(), Some("APP_ERROR"), "member {k}: {e}");
                let d = e.details().unwrap();
                assert_eq!(d["code"], "Taken", "member {k}: {e}");
                assert_eq!(d["fields"], json!({ "start": start.to_string() }));
                taken += 1;
            }
        }
    }
    assert_eq!(won.len(), 1, "winners: {won:?}");
    assert_eq!(taken, N - 1);
    let id = booking_id(&won[0].result);

    let mut owner_on = owner.on;
    for (i, cl) in owner_on.iter_mut().enumerate() {
        let v = cl
            .view(
                &app,
                "availability",
                json!({ "roomId": "r1", "dayStart": start.to_string() }),
                Consistency::Stale {
                    min_index: won[0].index,
                },
            )
            .await
            .unwrap();
        let slots = v.result.as_array().unwrap();
        assert_eq!(slots.len(), 1, "node {i}: {slots:?}");
        assert_eq!(slots[0]["bookingId"], json!(id), "node {i}");
        assert_eq!(slots[0]["start"], json!(start.to_string()), "node {i}");
    }
}

/// A5: after bob books, each node's replica (read with `minIndex` = the booking's index)
/// shows the slot to cy without its title, to ada (an admin) with it, and lists it in bob's
/// bookings.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn followers_honour_min_index_and_hide_titles() {
    let mut c = Cluster::start(3).await;
    let org = Org::setup(&c, &[("ada", "admin"), ("bob", "member"), ("cy", "member")]).await;
    let app = &org.app;
    let _r = org
        .on("ada", 0)
        .call(
            app,
            "addRoom",
            json!({ "id": "r1", "name": "Atlas", "capacity": 8 }),
            None,
        )
        .await
        .unwrap();
    let now = calimero_node::types::now_ms();
    let start = ((now / SLOT_MS + 100) * SLOT_MS).to_string();
    // A follower that misses the booking and is read right after it restarts, before it
    // caught up: only `minIndex` makes it wait.
    let leader = c.app_leader(app).unwrap();
    let lagging = (0..3).find(|i| *i != leader).unwrap();
    let writer = (0..3).find(|i| *i != lagging).unwrap();
    c.stop(lagging).await;
    let booked = org
        .on("bob", writer)
        .call(
            app,
            "book",
            json!({ "roomId": "r1", "start": start, "slotCount": 1, "title": "secret plan" }),
            None,
        )
        .await
        .unwrap();
    let id = booking_id(&booked.result);
    let at = Consistency::Stale {
        min_index: booked.index,
    };
    let day = json!({ "roomId": "r1", "dayStart": start });
    c.restart(lagging, |_| {}).await;
    // The precondition: the restarted follower has not applied the booking yet, so the reads
    // below prove that `minIndex` made it wait.
    let restarted = &c.nodes[lagging].as_ref().unwrap().node;
    let group = loop {
        if let Some(g) = restarted.hosted(app) {
            break g;
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    };
    let behind = group.applied();
    assert!(
        behind < booked.index,
        "node {lagging} already applied {behind} >= the booking's {} right after its restart; \
         the test no longer proves minIndex",
        booked.index
    );

    // The restarted node signs with a new key, so the session it issued no
    // longer verifies: the client refreshes, as a real one does on that 401.
    let _fresh = org.on("cy", lagging).refresh().await.unwrap();

    let order = std::iter::once(lagging).chain((0..3).filter(|i| *i != lagging));
    for node in order {
        let cy = org
            .on("cy", node)
            .view(app, "availability", day.clone(), at)
            .await
            .unwrap();
        assert!(
            cy.index >= booked.index,
            "node {node}: read at {}",
            cy.index
        );
        assert_eq!(
            cy.result,
            json!([{ "start": start, "end": (start.parse::<u64>().unwrap() + SLOT_MS).to_string(),
                     "bookingId": id, "title": null, "mine": false }]),
            "cy on node {node}"
        );

        let ada = org
            .on("ada", node)
            .view(app, "availability", day.clone(), at)
            .await
            .unwrap();
        let slots = ada.result.as_array().unwrap();
        assert_eq!(slots.len(), 1, "ada on node {node}: {slots:?}");
        assert_eq!(slots[0]["bookingId"], json!(id));
        assert_eq!(slots[0]["title"], "secret plan", "ada on node {node}");
        assert_eq!(slots[0]["mine"], false);

        let mine = org
            .on("bob", node)
            .view(app, "myBookings", json!({}), at)
            .await
            .unwrap();
        let ids: Vec<&Value> = mine
            .result
            .as_array()
            .unwrap()
            .iter()
            .map(|b| &b["id"])
            .collect();
        assert_eq!(ids, vec![&json!(id)], "bob on node {node}");
    }
}
