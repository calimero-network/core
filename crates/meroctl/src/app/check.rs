//! `meroctl app check`: what the people running an app will see when this release
//! replaces an earlier one, judged by the classifier the node judges updates with
//! (`calimero_bundle::update::classify`): green or red with the reasons, where the
//! unit's data can go (its destinations; none: it stays in Calimero), new and removed
//! methods, the state version and the signer. With `--fail-on`, a gate for a release workflow (exit [`EXIT_GATE`]).
//!
//! Both sides are units, as the node builds them for a release (`egress::release_unit`):
//! the release and each app its `uses` creates (mode `create`, transitively, at most
//! [`MAX_UNIT_DEPTH`] deep), each at the release its lock pinned, else the newest at hand
//! inside its range. A release is read from a signed `.mpk` (opened as a node opens it:
//! the signature and every hash) or, with a node, from its card
//! (`GET /packages/{package}/{version}/capabilities`), which carries the same signed facts.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use calimero_app_abi::load::wasm_abi;
use calimero_bundle::update::{classify, covers, Class, Classification, Reason, Release, Unit};
use calimero_bundle::{is_outbound_url, BundleManifest, BundleUse, UseMode, VerifiedBundle};
use clap::{Args, ValueEnum};
use eyre::{bail, eyre, WrapErr};
use serde_json::{json, Value};

use crate::env::Env;
use crate::make::PackArgs;

/// How deep the apps a release creates nest: the node's `MAX_UNIT_DEPTH`.
const MAX_UNIT_DEPTH: usize = 3;

/// The exit code when the gate (`--fail-on`) fails.
pub const EXIT_GATE: i32 = 1;
/// The exit code when the update could not be judged (a file that does not open, a
/// release that is not at hand, a node that refuses); clap's usage errors exit 2 too.
pub const EXIT_ERROR: i32 = 2;

#[derive(Debug, Args)]
pub struct CheckArgs {
    /// The candidate as a signed `.mpk`, instead of packing one.
    #[arg(long, conflicts_with_all = ["dir", "wasm", "compute", "services"])]
    pub mpk: Option<PathBuf>,
    /// The release it replaces: a signed `.mpk` (offline), or a version of the same
    /// package installed on the node (`--node`/`--api`), read from its card.
    #[arg(long, value_name = "VERSION|FILE.mpk")]
    pub against: String,
    /// A signed `.mpk` of an app either side creates with `uses` (repeatable): a member's
    /// release is taken from these first, then from the node.
    #[arg(long = "uses-mpk", value_name = "FILE.mpk")]
    pub uses_mpk: Vec<PathBuf>,
    /// The service whose ABI is compared, for a package of several services.
    #[arg(long, value_name = "NAME")]
    pub of_service: Option<String>,
    /// Print the full classification as JSON instead of the summary.
    #[arg(long)]
    pub json: bool,
    /// Exit 1 when the update is red (`red`), or only when its egress grows (`egress`:
    /// a new destination, or a member with egress joining), after `--allow-egress`.
    #[arg(long, value_enum, value_name = "red|egress")]
    pub fail_on: Option<FailOn>,
    /// A destination this release adds on purpose (repeatable, an `https://` prefix): a
    /// new destination it covers, and a joining member whose destinations it all covers,
    /// do not fail the gate.
    #[arg(long = "allow-egress", value_name = "PREFIX")]
    pub allow_egress: Vec<String>,
    // Last: its help heading covers the fields after it.
    /// The candidate: built and packed from DIR (default `.`) with `app pack`'s flags, as
    /// the release workflow packs it. Pass the release's `--key`: the signer is judged.
    #[command(flatten, next_help_heading = "Packing the candidate (as app pack)")]
    pub pack: PackArgs,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum FailOn {
    /// Any reason the update is red.
    Red,
    /// Only new egress: `egressAdded`, `memberAdded`.
    Egress,
}

impl CheckArgs {
    /// Whether it needs a node: only to read `--against VERSION`.
    pub fn needs_node(&self) -> bool {
        !against_is_file(&self.against)
    }
}

fn against_is_file(against: &str) -> bool {
    Path::new(against)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("mpk"))
        || Path::new(against).is_file()
}

/// `app check`: the summary (or `--json`) on stdout, then the exit code the module names.
/// Never returns an error: one that stops the judgement exits [`EXIT_ERROR`].
pub async fn run(a: CheckArgs, env: Option<&Env>) -> eyre::Result<Value> {
    let report = match check(a, env).await {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error: {e:?}");
            std::process::exit(EXIT_ERROR);
        }
    };
    println!("{}", report.out);
    std::io::stdout().flush()?;
    if !report.passed {
        std::process::exit(EXIT_GATE);
    }
    Ok(Value::Null)
}

struct Report {
    out: String,
    passed: bool,
}

async fn check(mut a: CheckArgs, env: Option<&Env>) -> eyre::Result<Report> {
    for p in &a.allow_egress {
        if !is_outbound_url(p) {
            bail!("{p}: an https:// prefix with a host, no credentials, query or fragment");
        }
    }
    let service = a.of_service.clone();
    let candidate = match a.mpk.take() {
        Some(mpk) => read_mpk(&read(&mpk)?, service.as_deref())
            .wrap_err_with(|| format!("reading {}", mpk.display()))?,
        None => read_mpk(&pack(&mut a.pack)?, service.as_deref())?,
    };
    let package = candidate.release.package.clone();
    let mut at_hand = AtHand {
        files: Vec::new(),
        env,
    };
    for file in &a.uses_mpk {
        at_hand.files.push(
            read_mpk(&read(file)?, None).wrap_err_with(|| format!("reading {}", file.display()))?,
        );
    }
    let previous = if against_is_file(&a.against) {
        let file = Path::new(&a.against);
        read_mpk(&read(file)?, service.as_deref())
            .wrap_err_with(|| format!("reading {}", file.display()))?
    } else {
        at_hand
            .on_node(&package, &a.against, service.as_deref())
            .await?
    };
    if previous.release.package != package {
        bail!(
            "--against {} is {}, the candidate is {package}",
            a.against,
            previous.release.package
        );
    }
    let before = at_hand.unit(previous).await?;
    let after = at_hand.unit(candidate).await?;
    let c = classify(&before, &after);
    let gate = gate(&c, a.fail_on, &a.allow_egress);
    let out = if a.json {
        serde_json::to_string_pretty(&json!({
            "package": package,
            "from": { "version": before.app.version, "signer": before.app.signer },
            "to": { "version": after.app.version, "signer": after.app.signer },
            "classification": c,
            "before": before,
            "after": after,
            "gate": {
                "failOn": a.fail_on,
                "allowEgress": a.allow_egress,
                "passed": gate.passed(),
                "failing": gate.failing,
                "waived": gate.waived,
            },
        }))?
    } else {
        summary(&Judged {
            before: &before,
            after: &after,
            c: &c,
            fail_on: a.fail_on,
            gate: &gate,
        })
    };
    Ok(Report {
        out,
        passed: gate.passed(),
    })
}

fn read(path: &Path) -> eyre::Result<Arc<[u8]>> {
    Ok(std::fs::read(path)
        .wrap_err_with(|| format!("reading {}", path.display()))?
        .into())
}

/// Packs the candidate as `app pack` would (DIR `.` when nothing names it), into a
/// temporary file unless `--out` names one, and answers its bytes.
fn pack(p: &mut PackArgs) -> eyre::Result<Arc<[u8]>> {
    if p.dir.is_none() && p.wasm.is_none() && p.compute.is_none() && p.services.is_empty() {
        p.dir = Some(PathBuf::from("."));
    }
    let tmp = tempfile::tempdir()?;
    let out = p
        .out
        .get_or_insert_with(|| tmp.path().join("candidate.mpk"))
        .clone();
    if p.key.is_none() {
        eprintln!("note: the candidate is signed with the development key; pass the release's --key to judge its signer");
    }
    let _wrote = crate::make::pack(p)?;
    read(&out)
}

/// A release as the classifier reads it, with the apps its `uses` names.
#[derive(Clone)]
struct Read {
    release: Release,
    uses: BTreeMap<String, BundleUse>,
}

/// A signed `.mpk`, opened as a node opens it; the ABI of `service`'s component (of its
/// only one when `None`), as the node picks it (`classifier_release`).
fn read_mpk(data: &Arc<[u8]>, service: Option<&str>) -> eyre::Result<Read> {
    let bundle = VerifiedBundle::open(Arc::clone(data), &semver::Version::new(u64::MAX, 0, 0))?;
    let contents = bundle.contents()?;
    let part = match service {
        Some(name) => Some(
            contents
                .wasm
                .iter()
                .find(|w| w.name.as_deref() == Some(name))
                .ok_or_else(|| eyre!("no service {name:?}"))?,
        ),
        None if contents.wasm.len() == 1 => contents.wasm.first(),
        None => None,
    };
    let abi = part
        .and_then(|w| w.bytes.as_deref().or(w.compute.as_deref()))
        .and_then(|bytes| wasm_abi(bytes).ok());
    let manifest = bundle.manifest();
    Ok(Read {
        release: Release::new(manifest, bundle.signer_id(), abi.as_ref()),
        uses: manifest.uses.clone().unwrap_or_default(),
    })
}

/// The releases `app check` can read: the `--uses-mpk` files, then the node's.
struct AtHand<'a> {
    files: Vec<Read>,
    env: Option<&'a Env>,
}

impl AtHand<'_> {
    /// Release `version` of `package` from the node's card of it.
    async fn on_node(
        &self,
        package: &str,
        version: &str,
        service: Option<&str>,
    ) -> eyre::Result<Read> {
        let Some(env) = self.env else {
            bail!("{package}@{version} is not at hand: pass its .mpk with --uses-mpk, or a node that has it installed with --against VERSION");
        };
        let card = env
            .client
            .packages()
            .capabilities(package, version, service)
            .await
            .wrap_err_with(|| format!("reading {package}@{version} from the node"))?;
        // The card's fields are the signed manifest's (`capability_card`).
        let mut manifest = json!({
            "version": "1.0",
            "package": card.package,
            "appVersion": card.version,
            "minRuntimeVersion": "0.0.0",
            "outbound": card.outbound,
        });
        for (key, value) in [("secrets", &card.secrets), ("uses", &card.uses)] {
            if !value.is_null() {
                manifest[key] = value.clone();
            }
        }
        let manifest: BundleManifest = serde_json::from_value(manifest)
            .wrap_err_with(|| format!("{package}@{version}'s card does not read"))?;
        let abi = (!card.abi.is_null()).then_some(&card.abi);
        Ok(Read {
            release: Release::new(&manifest, &card.signer, abi),
            uses: manifest.uses.clone().unwrap_or_default(),
        })
    }

    async fn release(&self, package: &str, version: &str) -> eyre::Result<Read> {
        if let Some(r) = self
            .files
            .iter()
            .find(|r| r.release.package == package && r.release.version == version)
        {
            return Ok(r.clone());
        }
        self.on_node(package, version, None).await
    }

    /// The release reference `name` creates: the one its lock pinned, else the newest at
    /// hand inside its range (as the node's `unit_members` picks among installed ones).
    async fn pick(&self, name: &str, u: &BundleUse) -> eyre::Result<String> {
        if let Some(v) = &u.version {
            return Ok(v.clone());
        }
        let range = semver::VersionReq::parse(&u.range)
            .wrap_err_with(|| format!("uses.{name}: range {}", u.range))?;
        let mut versions: Vec<String> = self
            .files
            .iter()
            .filter(|r| r.release.package == u.package)
            .map(|r| r.release.version.clone())
            .collect();
        if let Some(env) = self.env {
            match env.client.packages().get(&u.package).await {
                Ok(p) => versions.extend(p.versions.into_iter().map(|v| v.version)),
                Err(e) if e.code() == Some("NOT_FOUND") => {}
                Err(e) => return Err(e.into()),
            }
        }
        versions
            .iter()
            .filter_map(|v| semver::Version::parse(v).ok().map(|p| (p, v)))
            .filter(|(p, _)| range.matches(p))
            .max_by(|a, b| a.0.cmp(&b.0))
            .map(|(_, v)| v.clone())
            .ok_or_else(|| {
                eyre!(
                    "uses.{name}: no release of {} {} is at hand (the reference pins none): pass one with --uses-mpk",
                    u.package,
                    u.range
                )
            })
    }

    /// The unit of `root`: it and each release its `uses` creates, by path.
    async fn unit(&self, root: Read) -> eyre::Result<Unit> {
        let mut unit = Unit::of(root.release);
        let mut stack = vec![(String::new(), root.uses, 0_usize)];
        while let Some((path, uses, depth)) = stack.pop() {
            if depth >= MAX_UNIT_DEPTH {
                continue;
            }
            for (name, u) in uses {
                if u.mode != UseMode::Create {
                    continue;
                }
                let version = self.pick(&name, &u).await?;
                let member = self
                    .release(&u.package, &version)
                    .await
                    .wrap_err_with(|| format!("uses.{name} creates {}@{version}", u.package))?;
                let path = if path.is_empty() {
                    name
                } else {
                    format!("{path}/{name}")
                };
                let _prev = unit.members.insert(path.clone(), member.release);
                stack.push((path, member.uses, depth + 1));
            }
        }
        Ok(unit)
    }
}

/// What the gate makes of a classification: the reasons that fail it, and the
/// destinations `--allow-egress` waived.
struct Gate {
    on: Option<FailOn>,
    failing: Vec<Reason>,
    waived: Vec<String>,
}

impl Gate {
    fn passed(&self) -> bool {
        self.on.is_none() || self.failing.is_empty()
    }
}

fn gate(c: &Classification, on: Option<FailOn>, allow: &[String]) -> Gate {
    let allowed = |d: &String| allow.iter().any(|p| covers(p, d));
    let mut waived = Vec::new();
    let mut failing = Vec::new();
    for r in &c.reasons {
        let kept = match r {
            Reason::EgressAdded { destinations } => {
                let (ok, rest): (Vec<String>, Vec<String>) =
                    destinations.iter().cloned().partition(allowed);
                waived.extend(ok);
                (!rest.is_empty()).then_some(Reason::EgressAdded { destinations: rest })
            }
            Reason::MemberAdded { egress, .. } if egress.iter().all(allowed) => {
                waived.extend(egress.iter().cloned());
                None
            }
            Reason::MemberAdded { .. } => Some(r.clone()),
            _ if on == Some(FailOn::Egress) => None,
            _ => Some(r.clone()),
        };
        failing.extend(kept);
    }
    waived.sort();
    waived.dedup();
    if on.is_none() {
        failing.clear();
    }
    Gate {
        on,
        failing,
        waived,
    }
}

struct Judged<'a> {
    before: &'a Unit,
    after: &'a Unit,
    c: &'a Classification,
    fail_on: Option<FailOn>,
    gate: &'a Gate,
}

fn list(items: &[String]) -> String {
    if items.is_empty() {
        "none".to_owned()
    } else {
        items.join(", ")
    }
}

fn who(member: Option<&String>) -> String {
    member.map_or_else(|| "the app".to_owned(), |m| format!("member {m}"))
}

fn version(v: Option<u64>) -> String {
    v.map_or_else(|| "none".to_owned(), |v| v.to_string())
}

/// A red reason in plain words.
fn say(r: &Reason) -> String {
    match r {
        Reason::SignerChanged { member, from, to } => {
            format!("{} is signed by another key: {from} → {to}", who(member.as_ref()))
        }
        Reason::EgressAdded { destinations } => {
            format!("it can send data somewhere new: {}", list(destinations))
        }
        Reason::StateVersionChanged { member, from, to } => format!(
            "{} declares state version {} (was {}), and an update runs no migration",
            who(member.as_ref()),
            version(*to),
            version(*from)
        ),
        Reason::MemberAdded {
            member,
            package,
            egress,
        } => format!(
            "it now creates {member} ({package}), which sends data to {}",
            list(egress)
        ),
        Reason::MemberRemoved {
            member,
            package,
            egress,
        } => format!(
            "it no longer creates {member} ({package}, which sends data to {}): that app leaves this consent",
            list(egress)
        ),
        Reason::DelegatedMethodsAdded {
            member,
            package,
            methods,
        } => format!(
            "{} now calls {package}'s {} as the person",
            who(member.as_ref()),
            list(methods)
        ),
        Reason::Unclassified {
            member,
            package,
            version,
        } => format!(
            "{} was not classified: {package}@{version} is no longer installed",
            who(member.as_ref())
        ),
    }
}

/// The summary: the design's three lines, then egress, members, signer and the gate.
fn summary(j: &Judged<'_>) -> String {
    let c = j.c;
    let (before, after) = (j.before, j.after);
    let mut out = format!(
        "{} {} → {}\n",
        after.app.package, before.app.version, after.app.version
    );

    // Only destinations: an external endpoint, a TEE included, is listed like any other.
    out += &if c.egress.after.is_empty() {
        "  Data: stays in Calimero\n".to_owned()
    } else {
        format!("  Data can go to: {}\n", c.egress.after.join(", "))
    };

    // What a node does with it under an organization's `autoApply: noDestinations` policy
    // (`SystemState::auto_update_gate`): only a green update whose unit sends nothing out,
    // before or after, applies on its own, and it moves the app alone.
    let sends_out = !c.egress.before.is_empty() || !c.egress.after.is_empty();
    let members_move = before.members != after.members;
    out += match c.class {
        Class::Red => "  Users on automatic updates: wait for a person's approval (red):\n",
        Class::Green if sends_out => "  Users on automatic updates: wait for an owner's or admin's approval (green, but the app or the apps it created can send data out; only an update whose unit sends nothing out applies on its own)\n",
        Class::Green if members_move => "  Users on automatic updates: wait for an owner's or admin's approval (green, but it moves the apps it created, and an update on its own moves the app alone)\n",
        Class::Green => "  Users on automatic updates: apply it on their own after the hold where the policy is noDestinations (green: same signer, no new destination, same state version, no data leaves Calimero)\n",
    };
    if c.class == Class::Red {
        for r in &c.reasons {
            out += &format!("    - {}\n", say(r));
        }
    }

    let methods = match (&before.app.methods, &after.app.methods) {
        (Some(_), Some(_)) => format!(
            "New methods: {} · removed: {}",
            list(&c.new_methods),
            list(&c.removed_methods)
        ),
        _ => "Methods: unknown (a side embeds no ABI)".to_owned(),
    };
    let state = if before.app.state_version == after.app.state_version {
        format!(
            "state version unchanged ({})",
            version(after.app.state_version)
        )
    } else {
        format!(
            "state version {} → {}",
            version(before.app.state_version),
            version(after.app.state_version)
        )
    };
    out += &format!("  {methods} · {state}\n");

    out += &format!(
        "  Egress added: {} · removed: {}\n",
        list(&c.egress.added),
        list(&c.egress.removed)
    );

    let paths: BTreeSet<&String> = before.members.keys().chain(after.members.keys()).collect();
    if !paths.is_empty() {
        let members: Vec<String> = paths
            .into_iter()
            .map(|p| match (before.members.get(p), after.members.get(p)) {
                (Some(b), Some(a)) if b.package == a.package => {
                    format!("{p} {} {} → {}", a.package, b.version, a.version)
                }
                (Some(b), Some(a)) => format!(
                    "{p} {} {} → {} {}",
                    b.package, b.version, a.package, a.version
                ),
                (None, Some(a)) => format!("{p} {} {} (new)", a.package, a.version),
                (Some(b), None) => format!("{p} {} {} (gone)", b.package, b.version),
                (None, None) => unreachable!("a path of one side"),
            })
            .collect();
        out += &format!("  Members: {}\n", members.join(", "));
    }

    out += &if before.app.signer == after.app.signer {
        format!("  Signer: {} (unchanged)", after.app.signer)
    } else {
        format!("  Signer: {} (was {})", after.app.signer, before.app.signer)
    };

    if let Some(on) = j.fail_on {
        let flag = match on {
            FailOn::Red => "red",
            FailOn::Egress => "egress",
        };
        out += &format!("\nGate (--fail-on {flag}): ");
        if j.gate.failing.is_empty() {
            out += "passed";
        } else {
            out += "failed:";
            for r in &j.gate.failing {
                out += &format!("\n  - {}", say(r));
            }
        }
        if !j.gate.waived.is_empty() {
            out += &format!("\n  allowed by --allow-egress: {}", list(&j.gate.waived));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const AI: &str = "https://inference.near.ai/v1/";
    const ACME: &str = "https://api.acme.com/";

    fn release(version: &str, outbound: &[&str], methods: &[&str]) -> Release {
        Release {
            package: "com.acme.notes".into(),
            version: version.into(),
            signer: "did:key:a".into(),
            outbound: outbound.iter().map(|p| (*p).to_owned()).collect(),
            state_version: Some(1),
            methods: Some(methods.iter().map(|m| (*m).to_owned()).collect()),
            ..Release::default()
        }
    }

    fn judge(before: &Unit, after: &Unit, on: Option<FailOn>, allow: &[&str]) -> (String, bool) {
        let c = classify(before, after);
        let allow: Vec<String> = allow.iter().map(|p| (*p).to_owned()).collect();
        let gate = gate(&c, on, &allow);
        let text = summary(&Judged {
            before,
            after,
            c: &c,
            fail_on: on,
            gate: &gate,
        });
        (text, gate.passed())
    }

    #[test]
    fn a_green_update_says_where_data_goes_and_new_methods() {
        let before = Unit::of(release("1.3.0", &[AI], &["get"]));
        let after = Unit::of(release("1.4.0", &[AI], &["get", "pin", "unpin"]));
        let (text, passed) = judge(&before, &after, Some(FailOn::Red), &[]);
        assert!(passed);
        assert!(text.starts_with("com.acme.notes 1.3.0 → 1.4.0\n"), "{text}");
        assert!(
            text.contains("  Data can go to: https://inference.near.ai/v1/\n"),
            "{text}"
        );
        assert!(!text.to_lowercase().contains("sealed"), "{text}");
        // Green, but the unit sends data out: no policy applies it on its own.
        assert!(!text.contains("apply it on their own"), "{text}");
        assert!(
            text.contains(
                "  Users on automatic updates: wait for an owner's or admin's approval (green, but the app or the apps it created can send data out; only an update whose unit sends nothing out applies on its own)\n"
            ),
            "{text}"
        );
        assert!(
            text.contains("New methods: pin, unpin · removed: none · state version unchanged (1)"),
            "{text}"
        );
        assert!(text.contains("Signer: did:key:a (unchanged)"), "{text}");
        assert!(text.ends_with("Gate (--fail-on red): passed"), "{text}");
    }

    #[test]
    fn a_red_update_names_each_reason_and_fails_the_gate() {
        let before = Unit::of(release("1.3.0", &[AI], &["get"]));
        let mut to = release("1.4.0", &[AI, ACME], &["get"]);
        to.signer = "did:key:b".into();
        let after = Unit::of(to);
        let (text, passed) = judge(&before, &after, Some(FailOn::Red), &[]);
        assert!(!passed);
        assert!(
            text.contains(
                "  Data can go to: https://api.acme.com/, https://inference.near.ai/v1/\n"
            ),
            "{text}"
        );
        assert!(
            text.contains("wait for a person's approval (red)"),
            "{text}"
        );
        assert!(
            text.contains("- it can send data somewhere new: https://api.acme.com/"),
            "{text}"
        );
        assert!(
            text.contains("- the app is signed by another key: did:key:a → did:key:b"),
            "{text}"
        );
        assert!(text.contains("Signer: did:key:b (was did:key:a)"), "{text}");
        assert!(text.contains("Gate (--fail-on red): failed:"), "{text}");
    }

    #[test]
    fn allowed_egress_passes_the_gate_and_other_reasons_still_fail_it() {
        let before = Unit::of(release("1.3.0", &[AI], &["get"]));
        let after = Unit::of(release("1.4.0", &[AI, ACME], &["get"]));
        let (text, passed) = judge(&before, &after, Some(FailOn::Red), &[ACME]);
        assert!(passed, "{text}");
        assert!(
            text.contains("allowed by --allow-egress: https://api.acme.com/"),
            "{text}"
        );
        // A narrower prefix does not cover the wider destination.
        let (_, passed) = judge(
            &before,
            &after,
            Some(FailOn::Red),
            &["https://api.acme.com/v1/"],
        );
        assert!(!passed);

        let mut to = release("1.4.0", &[AI, ACME], &["get"]);
        to.state_version = Some(2);
        let after = Unit::of(to);
        let (_, passed) = judge(&before, &after, Some(FailOn::Red), &[ACME]);
        assert!(!passed, "a state version change is still red");
        let (_, passed) = judge(&before, &after, Some(FailOn::Egress), &[ACME]);
        assert!(passed, "--fail-on egress looks at egress only");
        let (_, passed) = judge(&before, &after, Some(FailOn::Egress), &[]);
        assert!(!passed);
        let (text, passed) = judge(&before, &after, None, &[]);
        assert!(passed, "no gate without --fail-on");
        assert!(!text.contains("Gate"), "{text}");
    }

    #[test]
    fn a_joining_member_with_allowed_egress_is_waived() {
        let before = Unit::of(release("1.3.0", &[], &["get"]));
        let mut after = Unit::of(release("1.4.0", &[], &["get"]));
        let mut ai = release("1.0.0", &[AI], &["chat"]);
        ai.package = "com.calimero.ai".into();
        let _prev = after.members.insert("ai".into(), ai);
        let (text, passed) = judge(&before, &after, Some(FailOn::Egress), &[]);
        assert!(!passed);
        assert!(
            text.contains("it now creates ai (com.calimero.ai), which sends data to https://inference.near.ai/v1/"),
            "{text}"
        );
        assert!(
            text.contains("Members: ai com.calimero.ai 1.0.0 (new)"),
            "{text}"
        );
        let (_, passed) = judge(
            &before,
            &after,
            Some(FailOn::Egress),
            &["https://inference.near.ai/"],
        );
        assert!(passed);
    }

    #[test]
    fn no_destinations_stay_in_calimero_and_methods_are_unknown_without_an_abi() {
        let before = Unit::of(release("1.3.0", &[ACME], &["get"]));
        let mut to = release("1.4.0", &[], &[]);
        to.methods = None;
        to.state_version = None;
        let after = Unit::of(to);
        let c = classify(&before, &after);
        let gate = gate(&c, None, &[]);
        let text = summary(&Judged {
            before: &before,
            after: &after,
            c: &c,
            fail_on: None,
            gate: &gate,
        });
        assert!(text.contains("  Data: stays in Calimero\n"), "{text}");
        assert!(
            text.contains("Egress added: none · removed: https://api.acme.com/"),
            "{text}"
        );
        assert!(text.contains("Methods: unknown"), "{text}");
        assert!(text.contains("state version 1 → none"), "{text}");
    }

    #[test]
    fn a_green_update_that_sends_nothing_out_may_apply_on_its_own_under_the_policy() {
        let before = Unit::of(release("1.3.0", &[], &["get"]));
        let after = Unit::of(release("1.4.0", &[], &["get", "pin"]));
        let (text, _) = judge(&before, &after, None, &[]);
        assert!(
            text.contains(
                "  Users on automatic updates: apply it on their own after the hold where the policy is noDestinations (green: same signer, no new destination, same state version, no data leaves Calimero)\n"
            ),
            "{text}"
        );

        // A destination before but none after: the policy still waits (before or after).
        let before = Unit::of(release("1.3.0", &[ACME], &["get"]));
        let (text, _) = judge(&before, &after, None, &[]);
        assert!(
            text.contains("wait for an owner's or admin's approval (green, but the app or the apps it created can send data out"),
            "{text}"
        );

        // Members that move: an update on its own moves the app alone.
        let mut before = Unit::of(release("1.3.0", &[], &["get"]));
        let mut after = Unit::of(release("1.4.0", &[], &["get"]));
        let mut store = release("1.0.0", &[], &["put"]);
        store.package = "com.acme.store".into();
        let _prev = before.members.insert("store".into(), store.clone());
        store.version = "1.1.0".into();
        let _prev = after.members.insert("store".into(), store);
        let (text, _) = judge(&before, &after, None, &[]);
        assert!(
            text.contains("  Users on automatic updates: wait for an owner's or admin's approval (green, but it moves the apps it created, and an update on its own moves the app alone)\n"),
            "{text}"
        );
    }

    #[test]
    fn against_is_a_file_or_a_version() {
        assert!(against_is_file("dist/notes-1.3.0.mpk"));
        assert!(against_is_file("prev.MPK"));
        assert!(!against_is_file("1.3.0"));
    }
}
