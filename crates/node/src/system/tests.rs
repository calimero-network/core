use serde_json::{json, Value};

use super::{
    Authority, Governance, OrgRole, Quotas, ReleaseScope, SystemCmd, SystemMachine, SystemState,
    INVITE_GONE, OPERATOR, QUOTA_PREFIX,
};
use crate::types::RefreshRecord;

// The tests above `account_with` judge each command's own authority rules
// (`run_authorized`); the proofs `run` also requires are tested below it.

fn account(id: &str) -> SystemCmd {
    SystemCmd::CreateAccount {
        at: 0,
        account: serde_json::from_value(
            json!({ "accountId": id, "name": id, "createdAt": 0, "passkeys": [] }),
        )
        .unwrap(),
        invite: None,
        refresh: None,
        consent: serde_json::Value::Null,
    }
}

fn genesis(governance: Governance) -> SystemCmd {
    SystemCmd::Genesis {
        session_proofs: true,
        governance,
        release: Some("2.0.0".into()),
        operator_key: None,
        nodes: Vec::new(),
    }
}

fn all(who: Authority) -> Governance {
    Governance {
        releases: who,
        app_upgrades: who,
        outbound: who,
        ..Governance::default()
    }
}

fn register(m: &mut SystemMachine, id: u64, role: &str) {
    let node =
        serde_json::from_value(json!({ "id": id, "addr": format!("http://n{id}"), "role": role }))
            .unwrap();
    m.run_authorized(SystemCmd::RegisterNode {
        node,
        proof: serde_json::Value::Null,
    })
    .unwrap();
}

#[test]
fn job_nodes_are_listed_apart_from_replicas() {
    let mut m = SystemMachine::default();
    register(&mut m, 1, "replica");
    register(&mut m, 2, "job");
    let ids = |it: Vec<&crate::types::NodeRecord>| it.iter().map(|n| n.id).collect::<Vec<_>>();
    assert_eq!(ids(m.state.replica_nodes().collect()), vec![1]);
    assert_eq!(ids(m.state.job_nodes().collect()), vec![2]);
    assert!(m.state.replica_node(&2).is_none());
    assert!(m.state.replica_node(&1).is_some());
    m.run_authorized(SystemCmd::RemoveNode {
        id: 2,
        proof: serde_json::Value::Null,
    })
    .unwrap();
    assert!(m.state.job_nodes().next().is_none());
}

#[test]
fn re_registering_a_node_never_changes_its_role() {
    let mut m = SystemMachine::default();
    register(&mut m, 1, "replica");
    register(&mut m, 2, "job");
    let node = |id: u64, addr: &str, role: &str| {
        serde_json::from_value(json!({ "id": id, "addr": addr, "role": role })).unwrap()
    };
    assert!(m
        .run_authorized(SystemCmd::RegisterNode {
            node: node(1, "u", "job"),
            proof: serde_json::Value::Null,
        })
        .is_err());
    assert!(m
        .run_authorized(SystemCmd::RegisterNode {
            node: node(2, "u", "replica"),
            proof: serde_json::Value::Null,
        })
        .is_err());
    assert_eq!(m.state.replica_nodes().count(), 1);
    assert_eq!(m.state.job_nodes().count(), 1);
    // The address of a node may change; so may anything after a removal.
    m.run_authorized(SystemCmd::RegisterNode {
        node: node(2, "v", "job"),
        proof: serde_json::Value::Null,
    })
    .unwrap();
    assert_eq!(m.state.nodes[&2].addr, "v");
    m.run_authorized(SystemCmd::RemoveNode {
        id: 2,
        proof: serde_json::Value::Null,
    })
    .unwrap();
    m.run_authorized(SystemCmd::RegisterNode {
        node: node(2, "v", "replica"),
        proof: serde_json::Value::Null,
    })
    .unwrap();
    assert_eq!(m.state.replica_nodes().count(), 2);
}

#[test]
fn addresses_are_compared_by_their_normal_form() {
    use super::normalize_addr as n;
    let one = n("http://127.0.0.1:2528/");
    for other in [
        "http://127.0.0.1:2528",
        "HTTP://127.0.0.1:2528/",
        "http://127.0.0.1:2528//",
    ] {
        assert_eq!(n(other), one, "{other}");
    }
    assert_eq!(n("http://Example.com:80/"), n("http://example.com"));
    assert_ne!(n("http://127.0.0.1:2529"), one);
    assert_ne!(n("https://127.0.0.1:2528"), one);
    assert_eq!(n("https://Example.com:443/"), n("https://example.com"));
    assert_eq!(n("http://[::1]:80/"), n("HTTP://[0:0:0:0:0:0:0:1]"));
    assert_ne!(n("http://[::1]:2528"), n("http://[::2]:2528"));
    // No origin to compare: the lowercased text, so distinct stay distinct.
    assert_ne!(n("a.example:80"), n("b.example:80"));
    assert_eq!(n("A.example:80/"), n("a.example:80"));
}

#[test]
fn a_node_cannot_take_an_address_another_node_holds() {
    let mut m = SystemMachine::default();
    register(&mut m, 1, "replica");
    let node = |id: u64, addr: &str| {
        serde_json::from_value(json!({ "id": id, "addr": addr, "role": "job" })).unwrap()
    };
    assert!(m
        .run_authorized(SystemCmd::RegisterNode {
            node: node(2, "HTTP://n1/"),
            proof: serde_json::Value::Null,
        })
        .is_err());
    m.run_authorized(SystemCmd::RegisterNode {
        node: node(2, "http://n2"),
        proof: serde_json::Value::Null,
    })
    .unwrap();
    // The holder may re-register its own address in another spelling.
    let own = serde_json::from_value(json!({ "id": 1, "addr": "http://N1/" })).unwrap();
    m.run_authorized(SystemCmd::RegisterNode {
        node: own,
        proof: serde_json::Value::Null,
    })
    .unwrap();
}

#[test]
fn a_nodes_epoch_moves_with_every_registration_and_removal() {
    let mut m = SystemMachine::default();
    let epoch = |m: &SystemMachine| m.state.node_epoch.get(&2).copied().unwrap_or(0);
    register(&mut m, 2, "job");
    let first = epoch(&m);
    register(&mut m, 2, "job");
    assert!(epoch(&m) > first);
    let second = epoch(&m);
    m.run_authorized(SystemCmd::RemoveNode {
        id: 2,
        proof: serde_json::Value::Null,
    })
    .unwrap();
    assert!(epoch(&m) > second);
}

#[test]
fn a_node_record_without_a_role_is_a_replica_and_an_unknown_role_is_refused() {
    let n: crate::types::NodeRecord =
        serde_json::from_value(json!({ "id": 1, "addr": "u" })).unwrap();
    assert_eq!(n.role, crate::types::NodeRole::Replica);
    assert!(serde_json::from_value::<crate::types::NodeRecord>(
        json!({ "id": 1, "addr": "u", "role": "gpu" })
    )
    .is_err());
}

/// The rules are written once, before anyone has an account, and an
/// operator who restarts the cluster with other ones changes nothing.
#[test]
fn governance_is_fixed_at_genesis() {
    let mut m = SystemMachine::default();
    assert!(
        m.run_authorized(account("alice")).is_err(),
        "an account before genesis"
    );
    let _ok = m.run_authorized(genesis(all(Authority::Operator))).unwrap();
    let _ok = m.run_authorized(account("alice")).unwrap();
    assert!(m.run_authorized(genesis(all(Authority::Owner))).is_err());
    assert_eq!(m.state.governance(), all(Authority::Operator));

    // Even before any account, a second genesis is refused.
    let mut m = SystemMachine::default();
    let _ok = m.run_authorized(genesis(all(Authority::Owner))).unwrap();
    assert!(m.run_authorized(genesis(all(Authority::Operator))).is_err());
    assert_eq!(m.state.governance(), all(Authority::Owner));

    // State from before governance reads as owner-decided throughout.
    assert_eq!(SystemState::default().governance(), all(Authority::Owner));
}

/// Each decision (releases, app upgrades, outbound hosts) belongs to
/// exactly one party, chosen independently at deployment: where the
/// operator holds it the owner cannot take it, and the other way round.
#[test]
fn each_decision_belongs_to_the_party_the_deployment_chose() {
    let both = [Authority::Owner, Authority::Operator];
    for releases in both {
        for app_upgrades in both {
            for outbound in both {
                let g = Governance {
                    releases,
                    app_upgrades,
                    outbound,
                    ..Governance::default()
                };
                check(&g);
            }
        }
    }
}

/// Leaving an open team sticks. Inheritance would put the
/// leaver straight back, so leaving would change nothing; they come back
/// only by an explicit act, a lead re-adding them, never by themselves.
#[test]
fn leaving_an_open_team_sticks_until_someone_lets_you_back_in() {
    let mut m = SystemMachine::default();
    let _ok = m.run_authorized(genesis(all(Authority::Owner))).unwrap();
    for who in ["alice", "bob"] {
        let _ok = m.run_authorized(account(who)).unwrap();
    }
    let _prev = m.state.orgs.insert(
        "org".into(),
        serde_json::from_value(json!({
            "orgId": "org", "name": "Docs",
            "members": { "alice": "owner", "bob": "member" },
            "teams": { "open": {
                "teamId": "open", "name": "Open", "join": "everyone",
                "members": { "alice": true, "bob": false }
            } }
        }))
        .unwrap(),
    );
    let reaches = |m: &SystemMachine| m.state.orgs["org"].in_team("bob", "open");
    let join = |actor: &str| SystemCmd::SetTeamMember {
        org: "org".into(),
        team: "open".into(),
        account: "bob".into(),
        lead: false,
        role: None,
        capabilities: None,
        actor: actor.into(),
        consent: serde_json::Value::Null,
    };
    assert!(reaches(&m), "a direct row");

    let _ok = m
        .run_authorized(SystemCmd::RemoveTeamMember {
            org: "org".into(),
            team: "open".into(),
            account: "bob".into(),
            actor: "bob".into(),
            consent: serde_json::Value::Null,
            subtree: true,
        })
        .unwrap();
    assert!(!reaches(&m), "inheritance does not put a leaver back");
    assert!(
        m.run_authorized(join("bob")).is_err(),
        "nor may they rejoin themselves"
    );

    let _ok = m.run_authorized(join("alice")).unwrap();
    assert!(reaches(&m), "a lead re-adds them");
}

/// `MANAGE_APPLICATION`: in an organization, an admin or a member
/// holding the capability (org-wide, or on the app's team) upgrades its
/// apps and approves its outbound hosts, as the org owner does. A personal
/// app stays its owner's, and a team-only grant reaches only that team.
#[test]
fn manage_application_lets_a_member_upgrade_and_approve_outbound() {
    const MANAGE_APPLICATION: u32 = 1 << 4;
    let mut m = SystemMachine::default();
    let _ok = m.run_authorized(genesis(all(Authority::Owner))).unwrap();
    for who in ["alice", "bob", "carol", "dave", "erin", "frank"] {
        let _ok = m.run_authorized(account(who)).unwrap();
    }
    let _prev = m.state.orgs.insert(
        "org".into(),
        serde_json::from_value(json!({
            "orgId": "org", "name": "Docs",
            "members": {
                "alice": "owner", "bob": "member", "carol": "member",
                "dave": "admin", "frank": "member"
            },
            "capabilities": { "bob": MANAGE_APPLICATION },
            "teams": { "t": {
                "teamId": "t", "name": "T", "join": "invite",
                "members": { "frank": false },
                "capabilities": { "frank": MANAGE_APPLICATION }
            } }
        }))
        .unwrap(),
    );
    for hash in ["b1", "b2"] {
        let _prev = m.state.bundles.insert(
            hash.into(),
            serde_json::from_value(json!({ "hash": hash, "size": 1, "uploadedBy": "alice" }))
                .unwrap(),
        );
    }
    let app = |id: &str, org: Option<&str>, access: serde_json::Value| {
        serde_json::from_value::<crate::types::AppRecord>(json!({
            "appId": id, "name": id, "group": 2, "bundle": "b1", "owner": "alice",
            "org": org, "access": access, "replicas": [1, 2, 3], "createdAt": 0
        }))
        .unwrap()
    };
    for (id, a) in [
        (
            "in-org",
            app("in-org", Some("org"), json!({ "kind": "org" })),
        ),
        (
            "in-team",
            app(
                "in-team",
                Some("org"),
                json!({ "kind": "team", "team": "t" }),
            ),
        ),
        (
            "personal",
            app("personal", None, json!({ "kind": "owner" })),
        ),
    ] {
        let _prev = m.state.apps.insert(id.into(), a);
    }
    let upgrade = |app: &str, actor: &str| SystemCmd::UpgradeApp {
        compute_bundle: None,
        app_id: app.into(),
        bundle: "b2".into(),
        by_operator: false,
        actor: actor.into(),
        at: 0,
        run: None,
        consent: serde_json::Value::Null,
        release: None,
        owner_in_org: true,
        owner_in_team: true,
    };
    for (app, actor, may) in [
        ("in-org", "bob", true),
        ("in-team", "bob", true),
        ("in-org", "dave", true),
        ("in-org", "alice", true),
        ("in-org", "carol", false),
        ("in-org", "erin", false),
        ("in-team", "frank", true),
        ("in-org", "frank", false),
        ("personal", "bob", false),
        ("personal", "dave", false),
    ] {
        assert_eq!(
            m.run_authorized(upgrade(app, actor)).is_ok(),
            may,
            "{actor} upgrading {app}"
        );
    }

    let _prev = m.state.packages.insert(
        "p".into(),
        serde_json::from_value(json!({
            "name": "p", "signerId": "did:key:zP", "owner": "alice",
            "versions": { "1.0.0": {
                "version": "1.0.0", "mpk": "00", "mpkSize": 1, "source": "upload",
                "services": { "": "b1" }, "outbound": ["https://api.example.com/v1/"],
                "installedBy": "alice", "installedAt": 0
            } }
        }))
        .unwrap(),
    );
    let outbound = |actor: &str| SystemCmd::SetOutboundApproval {
        scope: ReleaseScope::Org { org: "org".into() },
        package: "p".into(),
        selection: calimero_primitives::outbound::OutboundSelection::Manifest,
        replace: false,
        actor: actor.into(),
        consent: serde_json::Value::Null,
    };
    // An approval covers the whole organization, so a team-only grant
    // does not reach it.
    for (actor, may) in [
        ("bob", true),
        ("dave", true),
        ("alice", true),
        ("carol", false),
        ("frank", false),
        ("erin", false),
    ] {
        assert_eq!(
            m.run_authorized(outbound(actor)).is_ok(),
            may,
            "{actor} approving"
        );
    }
}

fn check(g: &Governance) {
    let op = |a: Authority| a == Authority::Operator;
    let mut m = SystemMachine::default();
    let _ok = m.run_authorized(genesis(g.clone())).unwrap();
    let _ok = m.run_authorized(account("alice")).unwrap();
    if let Some(a) = m.state.accounts.get_mut("alice") {
        let _new = a.approved_releases.insert("2.0.0".into());
    }
    for hash in ["b1", "b2"] {
        let _prev = m.state.bundles.insert(
            hash.into(),
            serde_json::from_value(json!({ "hash": hash, "size": 1, "uploadedBy": "alice" }))
                .unwrap(),
        );
    }
    let _prev = m.state.apps.insert(
        "app".into(),
        serde_json::from_value(json!({
            "appId": "app", "name": "app", "group": 2, "bundle": "b1",
            "owner": "alice", "replicas": [1, 2, 3], "createdAt": 0
        }))
        .unwrap(),
    );
    let _prev = m.state.packages.insert(
        "p".into(),
        serde_json::from_value(json!({
            "name": "p", "signerId": "did:key:zP", "owner": "alice",
            "versions": { "1.0.0": {
                "version": "1.0.0", "mpk": "00", "mpkSize": 1, "source": "upload",
                "services": { "": "b1" }, "outbound": ["https://api.example.com/v1/"],
                "installedBy": "alice", "installedAt": 0
            } }
        }))
        .unwrap(),
    );

    // App upgrades.
    let upgrade = |by_operator: bool, actor: &str| SystemCmd::UpgradeApp {
        compute_bundle: None,
        app_id: "app".into(),
        bundle: "b2".into(),
        by_operator,
        actor: actor.into(),
        at: 0,
        run: None,
        consent: serde_json::Value::Null,
        release: None,
        owner_in_org: true,
        owner_in_team: true,
    };
    let by_operator = m.run_authorized(upgrade(true, "")).is_ok();
    let by_owner = m.run_authorized(upgrade(false, "alice")).is_ok();
    assert_eq!(by_operator, op(g.app_upgrades), "{g:?}: operator upgrade");
    assert_eq!(by_owner, !op(g.app_upgrades), "{g:?}: owner upgrade");
    assert!(
        m.run_authorized(upgrade(false, "mallory")).is_err(),
        "{g:?}"
    );

    // Release approvals.
    let ops_release = m.run_authorized(SystemCmd::SetOperatorRelease {
        version: "2.1.0".into(),
        approved: true,
        proof: serde_json::Value::Null,
    });
    let owner_release = m.run_authorized(SystemCmd::SetReleaseApproval {
        scope: ReleaseScope::Account,
        version: "2.1.0".into(),
        approved: true,
        actor: "alice".into(),
        consent: serde_json::Value::Null,
    });
    assert_eq!(
        ops_release.is_ok(),
        op(g.releases),
        "{g:?}: operator release"
    );
    assert_eq!(
        owner_release.is_ok(),
        !op(g.releases),
        "{g:?}: owner release"
    );
    let app = m.state.apps["app"].clone();
    assert!(m.state.release_may_hold(&app, Some("2.0.0")), "{g:?}");
    assert!(m.state.release_may_hold(&app, Some("2.1.0")), "{g:?}");
    assert!(!m.state.release_may_hold(&app, Some("9.9.9")), "{g:?}");

    // Outbound approvals.
    let outbound = |scope: ReleaseScope, actor: &str| SystemCmd::SetOutboundApproval {
        scope,
        package: "p".into(),
        selection: calimero_primitives::outbound::OutboundSelection::Manifest,
        replace: false,
        actor: actor.into(),
        consent: serde_json::Value::Null,
    };
    let ops_outbound = m
        .run_authorized(outbound(ReleaseScope::Cluster, ""))
        .is_ok();
    let owner_outbound = m
        .run_authorized(outbound(ReleaseScope::Account, "alice"))
        .is_ok();
    assert_eq!(ops_outbound, op(g.outbound), "{g:?}: operator outbound");
    assert_eq!(owner_outbound, !op(g.outbound), "{g:?}: owner outbound");
    let approved = m
        .state
        .outbound_approval(&app, "did:key:zP")
        .map(|a| a.prefixes.clone());
    assert_eq!(
        approved,
        Some(vec!["https://api.example.com/v1/".to_owned()]),
        "{g:?}: the app reads the approval its deployment's party made"
    );
}

/// "Move this team to the top" is `parent: Some(None)`; it was lost on its
/// way through the log, where `null` read back as absent.
#[test]
fn moving_a_team_to_the_top_survives_the_log() {
    let update = |parent| SystemCmd::UpdateTeam {
        org: "o".into(),
        team: "t".into(),
        name: None,
        join: None,
        parent,
        metadata: None,
        actor: "a".into(),
        all_apps_rows: true,
        close_drops_below: true,
    };
    let round = |cmd: &SystemCmd| -> SystemCmd {
        serde_json::from_slice(&serde_json::to_vec(cmd).unwrap()).unwrap()
    };
    assert!(matches!(
        round(&update(Some(None))),
        SystemCmd::UpdateTeam {
            parent: Some(None),
            ..
        }
    ));
    assert!(matches!(
        round(&update(None)),
        SystemCmd::UpdateTeam { parent: None, .. }
    ));
    assert!(matches!(
        round(&update(Some(Some("p".into())))),
        SystemCmd::UpdateTeam {
            parent: Some(Some(_)),
            ..
        }
    ));
}

/// A release recorded before `installers` existed (system log entries and
/// snapshots are JSON) reads with none and writes back byte for byte as
/// before; installing the same release again records the new installer,
/// who then sees it, and the record restores from a snapshot with them.
#[test]
fn a_release_from_before_installers_reads_and_records_who_installs_it_again() {
    use super::{PackageVersion, OPERATOR};

    // `PackageVersion` as the previous release wrote it.
    let old = json!({
        "version": "1.0.0", "mpk": "ab", "mpkSize": 3, "source": "upload",
        "services": { "": "cd" }, "metadata": null, "outbound": [],
        "installedBy": "alice", "installedAt": 7,
    });
    let release: PackageVersion = serde_json::from_value(old.clone()).unwrap();
    assert!(release.installers.is_empty());
    assert_eq!(serde_json::to_value(&release).unwrap(), old);

    let mut m = SystemMachine::default();
    let _ok = m.run_authorized(genesis(all(Authority::Owner))).unwrap();
    for who in ["alice", "bob", "carol"] {
        let _ok = m.run_authorized(account(who)).unwrap();
    }
    let install = |by: &str| SystemCmd::InstallPackage {
        package: "p".into(),
        signer_id: "did:key:z".into(),
        release: PackageVersion {
            installed_by: by.into(),
            ..release.clone()
        },
    };
    let _ok = m.run_authorized(install("alice")).unwrap();
    let snapshot = serde_json::to_value(&m.state).unwrap();
    assert_eq!(snapshot["packages"]["p"]["versions"]["1.0.0"], old);
    let seen = |s: &SystemState, who: &str| s.sees_release(&s.packages["p"].versions["1.0.0"], who);
    assert!(seen(&m.state, "alice"));
    assert!(!seen(&m.state, "bob"), "another account, no shared org");
    assert!(!seen(&m.state, OPERATOR));

    // Installed again: by Bob (twice), then by the operator, which puts
    // it in the cluster's catalog; Alice stays first.
    let _ok = m.run_authorized(install("bob")).unwrap();
    let _ok = m.run_authorized(install("bob")).unwrap();
    assert!(seen(&m.state, "bob"));
    assert!(!seen(&m.state, "carol") && !seen(&m.state, OPERATOR));
    let _ok = m.run_authorized(install(OPERATOR)).unwrap();
    let v = &m.state.packages["p"].versions["1.0.0"];
    assert_eq!(v.installed_by, "alice");
    assert_eq!(
        v.installers.iter().collect::<Vec<_>>(),
        vec!["bob", OPERATOR]
    );
    assert!(seen(&m.state, OPERATOR) && seen(&m.state, "carol"));

    // Other bytes under the same version are still refused.
    let other = SystemCmd::InstallPackage {
        package: "p".into(),
        signer_id: "did:key:z".into(),
        release: PackageVersion {
            mpk: "ef".into(),
            installed_by: "carol".into(),
            ..release.clone()
        },
    };
    assert!(m.run_authorized(other).is_err());

    // A snapshot carries the installers; an old one restores without.
    let restored: SystemState =
        serde_json::from_value(serde_json::to_value(&m.state).unwrap()).unwrap();
    assert!(seen(&restored, "bob"));
    let old_state: SystemState = serde_json::from_value(snapshot).unwrap();
    assert!(old_state.packages["p"].versions["1.0.0"]
        .installers
        .is_empty());
    assert!(seen(&old_state, "alice") && !seen(&old_state, "bob"));
}

/// A release's compute component is recorded as the compute part of its service: no app
/// runs it, the app's record carries it next to `bundle` on create and upgrade, and
/// uninstalling the release takes it with the service.
#[test]
fn a_compute_component_is_recorded_beside_its_service_and_never_run_as_an_app() {
    use super::{BundleSource, PackageVersion};
    use calimero_bundle::UpgradeAuthority;
    use serde_json::Value;
    use std::collections::{BTreeMap, BTreeSet};
    let mut m = SystemMachine::default();
    let _ok = m.run_authorized(genesis(all(Authority::Owner))).unwrap();
    let _ok = m.run_authorized(account("alice")).unwrap();
    let release = |version: &str, main: &str, job: Option<&str>| SystemCmd::InstallPackage {
        package: "p".into(),
        signer_id: "did:key:z".into(),
        release: PackageVersion {
            version: version.into(),
            mpk: format!("mpk{version}"),
            mpk_size: 1,
            source: BundleSource::Upload,
            services: BTreeMap::from([(String::new(), main.to_owned())]),
            compute: job
                .map(|j| BTreeMap::from([(String::new(), j.to_owned())]))
                .unwrap_or_default(),
            part_sizes: BTreeMap::new(),
            metadata: Value::Null,
            outbound: Vec::new(),
            installed_by: "alice".into(),
            installed_at: 0,
            installers: BTreeSet::new(),
            upgrades: UpgradeAuthority::Owner,
            min_runtime_version: None,
        },
    };
    let _ok = m
        .run_authorized(release("1.0.0", "m1", Some("j1")))
        .unwrap();
    let _ok = m.run_authorized(release("1.1.0", "m2", None)).unwrap();
    let s = &m.state;
    assert!(s.bundles["j1"].package.as_ref().unwrap().compute);
    assert!(!s.bundles["m1"].package.as_ref().unwrap().compute);
    assert!(s.is_compute_bundle("j1") && !s.is_compute_bundle("m1"));
    assert!(!s.sees_bundle("j1", "alice") && s.sees_bundle("m1", "alice"));
    assert!(s.ships_compute("m1", Some("j1")) && s.ships_compute("m1", None));
    assert!(!s.ships_compute("m2", Some("j1")) && !s.ships_compute("m1", Some("m1")));

    let e = m
        .run_unproven(instance("bad", "alice", "j1", None))
        .unwrap_err();
    assert!(e.contains("compute component"), "{e}");
    let with_compute = |id: &str, compute: &str| SystemCmd::CreateApp {
        app: Box::new(
            serde_json::from_value(json!({
                "appId": id, "name": id, "group": 0, "bundle": "m1", "owner": "alice",
                "replicas": [1], "createdAt": 10, "computeBundle": compute,
            }))
            .unwrap(),
        ),
    };
    let e = m.run_unproven(with_compute("bad", "j2")).unwrap_err();
    assert!(e.contains("not shipped"), "{e}");
    let _ok = m.run_unproven(with_compute("app", "j1")).unwrap();
    assert_eq!(m.state.apps["app"].compute_bundle.as_deref(), Some("j1"));

    let upgrade = |bundle: &str, compute: Option<&str>| SystemCmd::UpgradeApp {
        compute_bundle: compute.map(str::to_owned),
        app_id: "app".into(),
        bundle: bundle.into(),
        by_operator: false,
        actor: "alice".into(),
        at: 0,
        run: None,
        consent: Value::Null,
        release: None,
        owner_in_org: true,
        owner_in_team: true,
    };
    let e = m.run_unproven(upgrade("j1", None)).unwrap_err();
    assert!(e.contains("compute component"), "{e}");
    let e = m.run_unproven(upgrade("m2", Some("j1"))).unwrap_err();
    assert!(e.contains("not shipped"), "{e}");
    assert_eq!(m.state.apps["app"].bundle, "m1");
    let _ok = m.run_unproven(upgrade("m2", None)).unwrap();
    assert_eq!(m.state.apps["app"].compute_bundle, None);

    let uninstall = |version: &str| SystemCmd::UninstallPackage {
        package: "p".into(),
        version: Some(version.into()),
        actor: "alice".into(),
    };
    let _ok = m.run_unproven(uninstall("1.0.0")).unwrap();
    assert!(!m.state.bundles.contains_key("m1") && !m.state.bundles.contains_key("j1"));
}

/// A release with a compute component counts each part's own size against the bundle
/// quota, not the whole `.mpk` once per part.
#[test]
fn a_compute_release_is_charged_each_parts_own_size() {
    use super::{BundleSource, PackageVersion};
    use calimero_bundle::UpgradeAuthority;
    use serde_json::Value;
    use std::collections::{BTreeMap, BTreeSet};
    let mut m = SystemMachine::default();
    let _ok = m.run_authorized(genesis(all(Authority::Owner))).unwrap();
    let _ok = m.run_authorized(account("alice")).unwrap();
    m.state.quotas.max_bundle_bytes_per_account = 37;
    let release = |version: &str, main: &str, job: &str| SystemCmd::InstallPackage {
        package: "p".into(),
        signer_id: "did:key:z".into(),
        release: PackageVersion {
            version: version.into(),
            mpk: format!("mpk{version}"),
            mpk_size: 40,
            source: BundleSource::Upload,
            services: BTreeMap::from([(String::new(), main.to_owned())]),
            compute: BTreeMap::from([(String::new(), job.to_owned())]),
            part_sizes: BTreeMap::from([(main.to_owned(), 30), (job.to_owned(), 7)]),
            metadata: Value::Null,
            outbound: Vec::new(),
            installed_by: "alice".into(),
            installed_at: 0,
            installers: BTreeSet::new(),
            upgrades: UpgradeAuthority::Owner,
            min_runtime_version: None,
        },
    };
    let _ok = m.run_authorized(release("1.0.0", "m1", "j1")).unwrap();
    assert_eq!(m.state.bundles["m1"].size, 30);
    assert_eq!(m.state.bundles["j1"].size, 7);
    let e = m.run_authorized(release("1.1.0", "m2", "j2")).unwrap_err();
    assert!(e.contains("quota exceeded"), "{e}");
}

/// An uninstall that would remove a compute component an app still runs is refused, like
/// one that would remove its main component.
#[test]
fn a_release_whose_compute_component_an_app_still_uses_is_not_uninstalled() {
    use super::{BundleSource, PackageVersion};
    use calimero_bundle::UpgradeAuthority;
    use serde_json::Value;
    use std::collections::{BTreeMap, BTreeSet};
    let mut m = SystemMachine::default();
    let _ok = m.run_authorized(genesis(all(Authority::Owner))).unwrap();
    let _ok = m.run_authorized(account("alice")).unwrap();
    let install = |version: &str, main: &str, job: &str| SystemCmd::InstallPackage {
        package: "p".into(),
        signer_id: "did:key:z".into(),
        release: PackageVersion {
            version: version.into(),
            mpk: format!("mpk{version}"),
            mpk_size: 1,
            source: BundleSource::Upload,
            services: BTreeMap::from([(String::new(), main.to_owned())]),
            compute: BTreeMap::from([(String::new(), job.to_owned())]),
            part_sizes: BTreeMap::new(),
            metadata: Value::Null,
            outbound: Vec::new(),
            installed_by: "alice".into(),
            installed_at: 0,
            installers: BTreeSet::new(),
            upgrades: UpgradeAuthority::Owner,
            min_runtime_version: None,
        },
    };
    let _ok = m.run_authorized(install("1.0.0", "m1", "j1")).unwrap();
    let _ok = m.run_authorized(install("1.1.0", "m1", "j2")).unwrap();
    // The app runs the main component both releases share, and the first one's compute.
    let app = SystemCmd::CreateApp {
        app: Box::new(
            serde_json::from_value(json!({
                "appId": "app", "name": "app", "group": 0, "bundle": "m1", "owner": "alice",
                "replicas": [1], "createdAt": 10, "computeBundle": "j1",
            }))
            .unwrap(),
        ),
    };
    let _ok = m.run_unproven(app).unwrap();
    let uninstall = |version: &str| SystemCmd::UninstallPackage {
        package: "p".into(),
        version: Some(version.into()),
        actor: "alice".into(),
    };
    let e = m.run_unproven(uninstall("1.0.0")).unwrap_err();
    assert!(e.contains("still runs"), "{e}");
    assert!(m.state.bundles.contains_key("j1"));
    let _ok = m.run_unproven(uninstall("1.1.0")).unwrap();
    assert!(m.state.bundles.contains_key("m1") && !m.state.bundles.contains_key("j2"));
}

/// An account `id` holding the passkey `pk`.
fn account_with(id: &str, pk: &mut calimero_identity::testing::SoftAuthenticator) -> SystemCmd {
    let rp = calimero_identity::RelyingParty {
        id: "localhost".into(),
        origins: vec!["http://localhost:3000".into()],
        require_uv: false,
    };
    let challenge = calimero_identity::random::<32>();
    let credential =
        calimero_identity::verify_registration(&rp, &challenge, &pk.register(&challenge)).unwrap();
    let account: crate::types::Account = serde_json::from_value(json!({
        "accountId": id, "name": id, "createdAt": 0,
        "passkeys": [credential], "approvedReleases": ["1.0.0"],
    }))
    .unwrap();
    let args = super::account_args(&account, None);
    SystemCmd::CreateAccount {
        at: 0,
        consent: consent(pk, &format!("account:{id}"), "__create_account", &args),
        account,
        invite: None,
        refresh: None,
    }
}

fn consent(
    pk: &mut calimero_identity::testing::SoftAuthenticator,
    subject: &str,
    method: &str,
    args: &serde_json::Value,
) -> serde_json::Value {
    let statement = crate::proof::Statement::new(subject, method, args);
    let challenge = statement.challenge().unwrap();
    serde_json::to_value(crate::proof::Consent {
        statement,
        assertion: pk.assert(&challenge),
        at: 0,
    })
    .unwrap()
}

fn approve_with(actor: &str, version: &str, consent: serde_json::Value) -> SystemCmd {
    SystemCmd::SetReleaseApproval {
        scope: ReleaseScope::Account,
        version: version.into(),
        approved: true,
        actor: actor.into(),
        consent,
    }
}

/// A cluster with alice and bob, each with a passkey.
fn two_accounts() -> (
    SystemMachine,
    calimero_identity::testing::SoftAuthenticator,
    calimero_identity::testing::SoftAuthenticator,
) {
    let mut m = SystemMachine::default();
    let _ok = m.run(genesis(all(Authority::Owner))).unwrap();
    let mut alice =
        calimero_identity::testing::SoftAuthenticator::new("localhost", "http://localhost:3000");
    let mut bob =
        calimero_identity::testing::SoftAuthenticator::new("localhost", "http://localhost:3000");
    let _ok = m.run(account_with("alice", &mut alice)).unwrap();
    let _ok = m.run(account_with("bob", &mut bob)).unwrap();
    (m, alice, bob)
}

/// A decision applies only with its actor's passkey
/// consent over exactly that decision, and only once.
#[test]
fn a_decision_applies_with_its_makers_consent_once() {
    let (mut m, mut alice, mut bob) = two_accounts();
    let args = json!({ "version": "2.0.0" });
    for (why, c) in [
        ("none", serde_json::Value::Null),
        ("an assertion alone, as before", {
            let mut c = consent(&mut alice, "account:alice", "__approve_release", &args);
            c["assertion"].take()
        }),
        (
            "bob's passkey",
            consent(&mut bob, "account:alice", "__approve_release", &args),
        ),
        (
            "bob's subject",
            consent(&mut alice, "account:bob", "__approve_release", &args),
        ),
        (
            "a withdrawal",
            consent(&mut alice, "account:alice", "__withdraw_release", &args),
        ),
        (
            "another release",
            consent(
                &mut alice,
                "account:alice",
                "__approve_release",
                &json!({ "version": "3.0.0" }),
            ),
        ),
    ] {
        assert!(m.run(approve_with("alice", "2.0.0", c)).is_err(), "{why}");
    }
    assert_eq!(m.state.accounts["alice"].approved_releases.len(), 1);

    let real = consent(&mut alice, "account:alice", "__approve_release", &args);
    let _ok = m.run(approve_with("alice", "2.0.0", real.clone())).unwrap();
    assert!(m.state.accounts["alice"]
        .approved_releases
        .contains("2.0.0"));
    // Withdrawn, then the old consent logged again: refused.
    let w = consent(&mut alice, "account:alice", "__withdraw_release", &args);
    let _ok = m
        .run(SystemCmd::SetReleaseApproval {
            scope: ReleaseScope::Account,
            version: "2.0.0".into(),
            approved: false,
            actor: "alice".into(),
            consent: w,
        })
        .unwrap();
    assert_eq!(
        m.run(approve_with("alice", "2.0.0", real)).unwrap_err(),
        "that consent was used already"
    );
    assert!(!m.state.accounts["alice"]
        .approved_releases
        .contains("2.0.0"));
}

/// An organization-wide run is consented to once; each app's upgrade
/// is then judged by the run, never by a consent of its own.
#[test]
fn an_upgrade_names_its_consent_or_its_run() {
    let (mut m, mut alice, _bob) = two_accounts();
    let upgrade = |consent: serde_json::Value, run: Option<&str>| SystemCmd::UpgradeApp {
        compute_bundle: None,
        app_id: "app".into(),
        bundle: "b2".into(),
        by_operator: false,
        actor: "alice".into(),
        at: 0,
        run: run.map(str::to_owned),
        consent,
        release: None,
        owner_in_org: true,
        owner_in_team: true,
    };
    assert!(super::authorize(&m.state, &upgrade(serde_json::Value::Null, None)).is_err());
    let c = consent(
        &mut alice,
        "app",
        "__upgrade",
        &json!({ "bundle": "b2", "migrate": null }),
    );
    assert!(super::authorize(&m.state, &upgrade(c, None))
        .unwrap()
        .is_some());
    let c = consent(&mut alice, "app", "__upgrade", &json!({ "bundle": "b3" }));
    assert!(super::authorize(&m.state, &upgrade(c, None)).is_err());
    // A run nobody started names nothing.
    assert!(super::authorize(&m.state, &upgrade(serde_json::Value::Null, Some("r1"))).is_err());
    let _ = &mut m;
}

type Soft = calimero_identity::testing::SoftAuthenticator;

/// The operator's signing key in these tests, and what genesis records.
fn operator_signer() -> (ed25519_dalek::SigningKey, crate::proof::OperatorKey) {
    let key = ed25519_dalek::SigningKey::from_bytes(&[7; 32]);
    let public = crate::proof::OperatorKey {
        key: hex::encode(key.verifying_key().to_bytes()),
        cluster: "test".into(),
    };
    (key, public)
}

/// The operator's signature over `method` on `subject` with `args`.
fn signed(subject: &str, method: &str, args: &serde_json::Value) -> serde_json::Value {
    use ed25519_dalek::Signer;
    let (key, public) = operator_signer();
    let statement = crate::proof::Statement::new(subject, method, args);
    let signature = key.sign(&statement.operator_message(&public.cluster));
    serde_json::to_value(crate::proof::OperatorProof {
        statement,
        signature: hex::encode(signature.to_bytes()),
    })
    .unwrap()
}

/// A cluster with the operator's key, alice (owner of `org`), bob (a
/// member) and carol (outside it), each with a passkey, a restricted team
/// `t`, and an app on nodes 1-3.
fn proven_cluster() -> (SystemMachine, Soft, Soft, Soft) {
    let mut m = SystemMachine::default();
    let _ok = m
        .run(SystemCmd::Genesis {
            session_proofs: true,
            governance: all(Authority::Owner),
            release: Some("2.0.0".into()),
            operator_key: Some(operator_signer().1),
            nodes: [1, 2, 3, 4]
                .map(|id| crate::types::NodeRecord {
                    id,
                    addr: format!("http://n{id}/"),
                    zone: String::new(),
                    role: crate::types::NodeRole::Replica,
                    pool: String::new(),
                })
                .to_vec(),
        })
        .unwrap();
    let mut pks = Vec::new();
    for who in ["alice", "bob", "carol"] {
        let mut pk = Soft::new("localhost", "http://localhost:3000");
        let _ok = m.run(account_with(who, &mut pk)).unwrap();
        pks.push(pk);
    }
    let _prev = m.state.orgs.insert(
        "org".into(),
        serde_json::from_value(json!({
            "orgId": "org", "name": "Docs",
            "members": { "alice": "owner", "bob": "member" },
            "teams": { "t": { "teamId": "t", "name": "T", "join": "invite", "members": {} } }
        }))
        .unwrap(),
    );
    let _prev = m.state.apps.insert(
        "app".into(),
        serde_json::from_value(json!({
            "appId": "app", "name": "app", "group": 2, "bundle": "b1",
            "owner": "alice", "replicas": [1, 2, 3], "createdAt": 0
        }))
        .unwrap(),
    );
    let carol = pks.pop().unwrap();
    let bob = pks.pop().unwrap();
    let alice = pks.pop().unwrap();
    (m, alice, bob, carol)
}

/// `cmd` applies only with `pk`'s consent over exactly `args`, and once:
/// refused without one, with one over `other`, and on replay.
fn needs_consent(
    m: &mut SystemMachine,
    pk: &mut Soft,
    (subject, method): (&str, &str),
    args: &serde_json::Value,
    other: &serde_json::Value,
    cmd: impl Fn(serde_json::Value) -> SystemCmd,
) {
    assert!(
        m.run(cmd(serde_json::Value::Null)).is_err(),
        "{method}: none"
    );
    let wrong = consent(pk, subject, method, other);
    assert!(m.run(cmd(wrong)).is_err(), "{method}: other args");
    let real = consent(pk, subject, method, args);
    if let Err(e) = m.run(cmd(real.clone())) {
        panic!("{method}: {e}");
    }
    assert_eq!(
        m.run(cmd(real)).unwrap_err(),
        "that consent was used already",
        "{method}: replay"
    );
}

/// As [`needs_consent`], for the operator's signature.
fn needs_signature(
    m: &mut SystemMachine,
    (subject, method): (&str, &str),
    args: &serde_json::Value,
    other: &serde_json::Value,
    cmd: impl Fn(serde_json::Value) -> SystemCmd,
) {
    assert!(
        m.run(cmd(serde_json::Value::Null)).is_err(),
        "{method}: none"
    );
    assert!(
        m.run(cmd(signed(subject, method, other))).is_err(),
        "{method}: other args"
    );
    let mut forged = signed(subject, method, args);
    forged["signature"] = json!(hex::encode([0_u8; 64]));
    assert!(m.run(cmd(forged)).is_err(), "{method}: not the operator's");
    let real = signed(subject, method, args);
    if let Err(e) = m.run(cmd(real.clone())) {
        panic!("{method}: {e}");
    }
    assert_eq!(
        m.run(cmd(real)).unwrap_err(),
        "that consent was used already",
        "{method}: replay"
    );
}

/// Credentials an account gets (passkeys, API keys) apply only with a
/// passkey it already has, over exactly that credential, once.
#[test]
fn credentials_need_the_accounts_own_passkey() {
    let (mut m, mut alice, mut bob, _carol) = proven_cluster();
    let mut phone = Soft::new("localhost", "http://localhost:3000");
    let rp = calimero_identity::RelyingParty {
        id: "localhost".into(),
        origins: vec!["http://localhost:3000".into()],
        require_uv: false,
    };
    let challenge = calimero_identity::random::<32>();
    let cred = calimero_identity::verify_registration(&rp, &challenge, &phone.register(&challenge))
        .unwrap();
    let add = |consent| SystemCmd::AddPasskey {
        at: 0,
        account: "alice".into(),
        credential: cred.clone(),
        consent,
    };
    let args = json!({ "credentialId": cred.id, "publicKey": cred.public_key });
    // Neither the new passkey itself nor another account's approves it.
    for pk in [&mut phone, &mut bob] {
        let c = consent(pk, "account:alice", "__add_passkey", &args);
        assert!(m.run(add(c)).is_err());
    }
    let other = json!({ "credentialId": cred.id, "publicKey": "AAAA" });
    needs_consent(
        &mut m,
        &mut alice,
        ("account:alice", "__add_passkey"),
        &args,
        &other,
        add,
    );
    assert_eq!(m.state.accounts["alice"].passkeys.len(), 2);

    let remove = |consent| SystemCmd::RemovePasskey {
        account: "alice".into(),
        credential_id: cred.id.clone(),
        now: 1,
        consent,
    };
    needs_consent(
        &mut m,
        &mut alice,
        ("account:alice", "__remove_passkey"),
        &json!({ "credentialId": cred.id }),
        &json!({ "credentialId": "another" }),
        remove,
    );
    assert_eq!(m.state.accounts["alice"].passkeys.len(), 1);

    let record: crate::types::ApiKeyRecord = serde_json::from_value(json!({
        "keyId": "k1", "account": "alice", "label": "ci", "apps": ["app"],
        "methods": null, "readOnly": true, "allowConfirm": false, "expiresAt": 99,
    }))
    .unwrap();
    let args = super::api_key_args("h1", &record);
    let mut wider = args.clone();
    wider["readOnly"] = json!(false);
    let create = |consent| SystemCmd::CreateApiKey {
        hash: "h1".into(),
        record: record.clone(),
        consent,
        owner_in_org: true,
        owner_in_team: true,
    };
    // Another key's hash under the same consent: refused.
    let c = consent(&mut alice, "account:alice", "__create_api_key", &args);
    assert!(m
        .run(SystemCmd::CreateApiKey {
            hash: "h2".into(),
            record: record.clone(),
            consent: c,
            owner_in_org: true,
            owner_in_team: true,
        })
        .is_err());
    needs_consent(
        &mut m,
        &mut alice,
        ("account:alice", "__create_api_key"),
        &args,
        &wider,
        create,
    );
    assert!(m.state.api_keys.contains_key("h1"));
}

/// A cluster begun before session proofs replays what it logged then as
/// it applied it, proofless, and requires the proofs from the entry its
/// first leader on this release wrote; genesis requires them from the
/// start. The mark is written once and only tightens.
#[test]
fn session_commands_logged_before_their_proofs_apply_as_they_did() {
    let mut m = SystemMachine::default();
    m.state.governance = Some(Governance::default());
    m.index = 3;
    let _ok = m.run(account("alice")).unwrap();
    m.index = 5;
    let _ok = m.run(SystemCmd::RequireSessionProofs).unwrap();
    assert_eq!(m.state.session_proofs_from, Some(5));
    assert!(m.run(SystemCmd::RequireSessionProofs).is_err(), "once");
    m.index = 6;
    assert!(m.run(account("bob")).is_err(), "proofless after the mark");
    // A restart from a snapshot before the mark replays what came before it.
    m.index = 4;
    let _ok = m.run(account("carol")).unwrap();

    let mut fresh = SystemMachine {
        index: 1,
        ..SystemMachine::default()
    };
    let _ok = fresh.run(genesis(all(Authority::Owner))).unwrap();
    assert_eq!(fresh.state.session_proofs_from, Some(1));
    fresh.index = 2;
    assert!(
        fresh.run(account("alice")).is_err(),
        "required from genesis"
    );
    // State from before reads without the mark and writes as it did.
    let before = serde_json::to_value(SystemState::default()).unwrap();
    assert!(before.get("sessionProofsFrom").is_none());
}

/// A refresh token for `account`'s `passkey`, as the API makes one: the
/// token, its hash, and its record.
fn refresh_token(
    account: &str,
    passkey: &str,
    session: &str,
) -> (String, String, crate::types::RefreshRecord) {
    let token = format!(
        "rt_{}",
        calimero_identity::b64url(&calimero_identity::random::<32>())
    );
    let hash = calimero_identity::b64url(&calimero_identity::sha256(token.as_bytes()));
    let record = crate::types::RefreshRecord {
        account: account.into(),
        session: session.into(),
        expires_at: 99,
        passkey: Some(passkey.into()),
        app: None,
        origin: None,
        key: crate::proof::refresh_key(&token),
        opened_at: 0,
    };
    (token, hash, record)
}

/// A sign-in's refresh token applies only with that sign-in's assertion,
/// by the passkey its record names, over a challenge stating that token;
/// once.
#[test]
fn a_sign_ins_refresh_token_needs_its_passkeys_assertion() {
    let (mut m, mut alice, mut bob, _carol) = proven_cluster();
    let (_token, hash, record) = refresh_token("alice", &alice.credential_id(), "s1");
    let create = |consent| SystemCmd::CreateRefresh {
        hash: hash.clone(),
        record: record.clone(),
        consent,
    };
    let args = super::sign_in_args(&hash, &record);
    // Bob's passkey does not sign alice in, nor does a record naming bob's
    // passkey take alice's assertion.
    let c = consent(&mut bob, "session", "__sign_in", &args);
    assert!(m.run(create(c)).is_err());
    let c = consent(&mut alice, "session", "__sign_in", &args);
    let mut bobs = record.clone();
    bobs.passkey = Some(bob.credential_id());
    assert!(m
        .run(SystemCmd::CreateRefresh {
            hash: hash.clone(),
            record: bobs,
            consent: c,
        })
        .is_err());
    // An assertion over another token: refused.
    let (_t, other_hash, other) = refresh_token("alice", &alice.credential_id(), "s1");
    let c = consent(
        &mut alice,
        "session",
        "__sign_in",
        &super::sign_in_args(&other_hash, &other),
    );
    assert!(m.run(create(c)).is_err());
    needs_consent(
        &mut m,
        &mut alice,
        ("session", "__sign_in"),
        &args,
        &json!({ "refresh": hash, "key": "00", "session": "s1", "expiresAt": 99 }),
        create,
    );
    assert_eq!(m.state.refresh[&hash].account, "alice");
}

/// A refresh token is exchanged only with its own key's signature, for
/// the session it already belongs to.
#[test]
fn a_refresh_token_is_exchanged_with_its_own_key() {
    let (mut m, mut alice, _bob, _carol) = proven_cluster();
    let (token, hash, record) = refresh_token("alice", &alice.credential_id(), "s1");
    let c = consent(
        &mut alice,
        "session",
        "__sign_in",
        &super::sign_in_args(&hash, &record),
    );
    let _ok = m
        .run(SystemCmd::CreateRefresh {
            hash: hash.clone(),
            record: record.clone(),
            consent: c,
        })
        .unwrap();
    let (next, next_hash, next_record) = refresh_token("alice", &alice.credential_id(), "s1");
    let message = |r: &crate::types::RefreshRecord| {
        crate::proof::refresh_use_message(&hash, &next_hash, &r.key, &r.session, r.expires_at, 5)
    };
    let exchange = |record: crate::types::RefreshRecord, signature: String| SystemCmd::UseRefresh {
        old_hash: hash.clone(),
        new_hash: next_hash.clone(),
        record,
        now: 5,
        signature,
    };
    for (why, cmd) in [
        ("unsigned", exchange(next_record.clone(), String::new())),
        (
            "signed with another token",
            exchange(
                next_record.clone(),
                crate::proof::sign_refresh_use(&next, &message(&next_record)),
            ),
        ),
        (
            "into another session",
            exchange(
                crate::types::RefreshRecord {
                    session: "s2".into(),
                    ..next_record.clone()
                },
                crate::proof::sign_refresh_use(
                    &token,
                    &message(&crate::types::RefreshRecord {
                        session: "s2".into(),
                        ..next_record.clone()
                    }),
                ),
            ),
        ),
        (
            "signed over another expiry",
            exchange(
                next_record.clone(),
                crate::proof::sign_refresh_use(
                    &token,
                    &message(&crate::types::RefreshRecord {
                        expires_at: 1,
                        ..next_record.clone()
                    }),
                ),
            ),
        ),
        // A session keeps the registered origin it was opened on, so
        // removing the origin still ends it after an exchange.
        (
            "onto another origin",
            exchange(
                crate::types::RefreshRecord {
                    origin: Some("https://elsewhere.example.com".into()),
                    ..next_record.clone()
                },
                crate::proof::sign_refresh_use(&token, &message(&next_record)),
            ),
        ),
    ] {
        assert!(m.run(cmd).is_err(), "{why}");
    }
    assert!(m.state.refresh.contains_key(&hash));
    let signed = crate::proof::sign_refresh_use(&token, &message(&next_record));
    let _ok = m
        .run(exchange(next_record.clone(), signed.clone()))
        .unwrap();
    assert!(m.state.refresh.contains_key(&next_hash));
    assert!(m.run(exchange(next_record, signed)).is_err(), "replayed");

    // A token from before keys is not exchanged at all.
    let (old, old_hash, mut keyless) = refresh_token("alice", &alice.credential_id(), "s3");
    keyless.key = String::new();
    let _prev = m.state.refresh.insert(old_hash.clone(), keyless.clone());
    let (_n, new_hash, new_record) = refresh_token("alice", &alice.credential_id(), "s3");
    let message = crate::proof::refresh_use_message(
        &old_hash,
        &new_hash,
        &new_record.key,
        "s3",
        new_record.expires_at,
        5,
    );
    assert_eq!(
        m.run(SystemCmd::UseRefresh {
            old_hash,
            new_hash,
            record: new_record,
            now: 5,
            signature: crate::proof::sign_refresh_use(&old, &message),
        })
        .unwrap_err(),
        "this refresh token has no key; sign in again"
    );
}

/// Passkey and session times come from the entries: a sign-up's and an
/// added passkey's `at`, a sign-in's `opened_at`. The latest sign-in wins
/// whatever order they commit in, and an exchange keeps when its session
/// opened. Entries an earlier release logged carry none, and leave the
/// times unknown on every replica instead of making one up.
#[test]
fn passkey_and_session_times_come_from_their_entries() {
    let (mut m, mut alice, _bob, _carol) = proven_cluster();
    let cred = alice.credential_id();
    // `proven_cluster`'s sign-ups are an earlier release's: no `at`.
    assert!(m.state.accounts["alice"].passkey_times.is_empty());
    let mut sign_in = |m: &mut SystemMachine, session: &str, opened_at: u64| {
        let (token, hash, record) = refresh_token("alice", &cred, session);
        let record = crate::types::RefreshRecord {
            opened_at,
            ..record
        };
        let c = consent(
            &mut alice,
            "session",
            "__sign_in",
            &super::sign_in_args(&hash, &record),
        );
        let _ok = m
            .run(SystemCmd::CreateRefresh {
                hash: hash.clone(),
                record,
                consent: c,
            })
            .unwrap();
        (token, hash)
    };
    let _old = sign_in(&mut m, "s0", 0);
    assert!(
        m.state.accounts["alice"].passkey_times.is_empty(),
        "a sign-in an earlier release logged moves nothing"
    );
    let (token, hash) = sign_in(&mut m, "s1", 40);
    let _later = sign_in(&mut m, "s2", 50);
    let _earlier = sign_in(&mut m, "s3", 45);
    assert_eq!(
        m.state.accounts["alice"].passkey_times[&cred],
        crate::types::PasskeyTimes {
            added_at: None,
            last_sign_in_at: Some(50),
        },
        "added before it was kept; the latest sign-in wins"
    );
    assert_eq!(m.state.refresh[&hash].opened_at, 40);

    // An exchange that states no opening (an earlier release's) keeps
    // the session's.
    let (_next, next_hash, next_record) = refresh_token("alice", &cred, "s1");
    let signed = crate::proof::sign_refresh_use(
        &token,
        &crate::proof::refresh_use_message(
            &hash,
            &next_hash,
            &next_record.key,
            "s1",
            next_record.expires_at,
            5,
        ),
    );
    let _ok = m
        .run(SystemCmd::UseRefresh {
            old_hash: hash,
            new_hash: next_hash.clone(),
            record: next_record,
            now: 5,
            signature: signed,
        })
        .unwrap();
    assert_eq!(m.state.refresh[&next_hash].opened_at, 40);
    assert_eq!(
        m.state.accounts["alice"].passkey_times[&cred].last_sign_in_at,
        Some(50),
        "a refresh is no sign-in"
    );

    // A sign-up on this release: added and signed in at its `at`, and
    // its session opened then.
    let mut dave = Soft::new("localhost", "http://localhost:3000");
    let SystemCmd::CreateAccount { account, .. } = account_with("dave", &mut dave) else {
        unreachable!()
    };
    let (_t, dave_hash, record) = refresh_token("dave", &dave.credential_id(), "d1");
    let grant = super::RefreshGrant {
        hash: dave_hash.clone(),
        record,
    };
    let args = super::account_args(&account, Some(&grant));
    let _ok = m
        .run(SystemCmd::CreateAccount {
            consent: consent(&mut dave, "account:dave", "__create_account", &args),
            account,
            invite: None,
            refresh: Some(grant),
            at: 70,
        })
        .unwrap();
    assert_eq!(
        m.state.accounts["dave"].passkey_times[&dave.credential_id()],
        crate::types::PasskeyTimes {
            added_at: Some(70),
            last_sign_in_at: Some(70),
        }
    );
    assert_eq!(m.state.refresh[&dave_hash].opened_at, 70);
}

/// An account and a refresh record as an earlier release wrote them read
/// with their times unknown and write back byte for byte, and so does
/// the account a sign-up's passkey signs (`account_args`), so a replica
/// on either release checks the same consent.
#[test]
fn accounts_and_sessions_from_before_the_times_read_and_write_as_they_did() {
    let account = json!({
        "accountId": "alice", "name": "alice", "createdAt": 3,
        "passkeys": [{ "id": "c1", "public_key": { "alg": "Es256", "key": "pk" }, "sign_count": 0 }],
        "approvedReleases": ["1.0.0"], "outboundApprovals": {},
        "passkeyLabels": { "c1": "laptop" }, "aliases": {},
    });
    let read: crate::types::Account = serde_json::from_value(account.clone()).unwrap();
    assert!(read.passkey_times.is_empty());
    assert_eq!(serde_json::to_value(&read).unwrap(), account);
    let record = json!({
        "account": "alice", "session": "s1", "expiresAt": 99, "passkey": "c1", "key": "ab",
    });
    let read: crate::types::RefreshRecord = serde_json::from_value(record.clone()).unwrap();
    assert_eq!(read.opened_at, 0);
    assert_eq!(serde_json::to_value(&read).unwrap(), record);
    // An entry an earlier release logged replays as the same command.
    let entry = json!({ "op": "addPasskey", "account": "alice",
            "credential": { "id": "c2", "public_key": { "alg": "Es256", "key": "pk" }, "sign_count": 0 }, "consent": null });
    let cmd: SystemCmd = serde_json::from_value(entry.clone()).unwrap();
    assert!(matches!(cmd, SystemCmd::AddPasskey { at: 0, .. }));
    assert_eq!(serde_json::to_value(&cmd).unwrap(), entry);
}

/// An account's activity comes from the entries' own times: a sign-in's
/// session opening (with the app and origin it is bound to) and a
/// step-up's consent time, under the passkey that gave it. A repeated
/// sign-in moves the newest entry's time; a refused command, a consent
/// with no time (an earlier release's) and an earlier release's sign-in
/// add nothing; the oldest entries go past the cap; nobody else's
/// activity moves.
#[test]
fn activity_records_sign_ins_and_step_ups_from_their_times() {
    use crate::types::{Activity, ActivityEntry, ACTIVITY_KEPT};
    let (mut m, mut alice, _bob, _carol) = proven_cluster();
    let cred = alice.credential_id();
    assert!(m
        .state
        .activity
        .get("alice")
        .cloned()
        .unwrap_or_default()
        .is_empty());
    let mut session = 0;
    let mut sign_in =
        |m: &mut SystemMachine, alice: &mut Soft, opened_at: u64, app: Option<&str>| {
            session += 1;
            let (_token, hash, record) = refresh_token("alice", &cred, &format!("s{session}"));
            let record = crate::types::RefreshRecord {
                opened_at,
                app: app.map(str::to_owned),
                origin: app.map(|a| format!("https://{a}.example.com")),
                ..record
            };
            let c = consent(
                alice,
                "session",
                "__sign_in",
                &super::sign_in_args(&hash, &record),
            );
            let _ok = m
                .run(SystemCmd::CreateRefresh {
                    hash,
                    record,
                    consent: c,
                })
                .unwrap();
        };
    let timed = |c: serde_json::Value, at: u64| {
        let mut c = c;
        c["at"] = json!(at);
        c
    };
    let signed_in = |at: u64, app: Option<&str>| ActivityEntry {
        at,
        passkey: cred.clone(),
        what: Activity::SignIn {
            app: app.map(str::to_owned),
            origin: app.map(|a| format!("https://{a}.example.com")),
        },
    };

    sign_in(&mut m, &mut alice, 0, None);
    assert!(
        m.state
            .activity
            .get("alice")
            .cloned()
            .unwrap_or_default()
            .is_empty(),
        "an earlier release's sign-in adds nothing"
    );
    sign_in(&mut m, &mut alice, 40, Some("com.notes"));
    sign_in(&mut m, &mut alice, 45, Some("com.notes"));
    assert_eq!(
        m.state.activity.get("alice").cloned().unwrap_or_default(),
        [signed_in(45, Some("com.notes"))],
        "a repeated sign-in moves the newest entry's time"
    );

    // A step-up: its method and subject, never its arguments.
    let args = json!({ "version": "2.0.0" });
    let wrong = timed(
        consent(&mut alice, "account:bob", "__approve_release", &args),
        50,
    );
    assert!(m.run(approve_with("alice", "2.0.0", wrong)).is_err());
    let real = timed(
        consent(&mut alice, "account:alice", "__approve_release", &args),
        50,
    );
    let _ok = m.run(approve_with("alice", "2.0.0", real)).unwrap();
    let approved = ActivityEntry {
        at: 50,
        passkey: cred.clone(),
        what: Activity::Approval {
            method: "__approve_release".into(),
            subject: "account:alice".into(),
        },
    };
    assert_eq!(
        m.state.activity.get("alice").cloned().unwrap_or_default(),
        [signed_in(45, Some("com.notes")), approved.clone()],
        "a refused command adds nothing"
    );
    // A consent with no time (an earlier release's) adds nothing.
    let untimed = consent(&mut alice, "account:alice", "__withdraw_release", &args);
    let _ok = m
        .run(SystemCmd::SetReleaseApproval {
            scope: ReleaseScope::Account,
            version: "2.0.0".into(),
            approved: false,
            actor: "alice".into(),
            consent: untimed,
        })
        .unwrap();
    assert_eq!(
        m.state
            .activity
            .get("alice")
            .cloned()
            .unwrap_or_default()
            .len(),
        2
    );
    // Stored as the route lists it, and left out when empty.
    assert_eq!(
        serde_json::to_value(&approved).unwrap(),
        json!({ "at": 50, "passkey": cred, "kind": "approval",
                "method": "__approve_release", "subject": "account:alice" })
    );
    assert!(serde_json::to_value(SystemState::default())
        .unwrap()
        .get("activity")
        .is_none());

    // Past the cap the oldest go first.
    for i in 0..ACTIVITY_KEPT {
        let app = if i % 2 == 0 { "com.a" } else { "com.b" };
        sign_in(&mut m, &mut alice, 100 + i as u64, Some(app));
    }
    let kept = &m.state.activity.get("alice").cloned().unwrap_or_default();
    assert_eq!(kept.len(), ACTIVITY_KEPT);
    assert!(!kept.contains(&approved), "the oldest went first");
    assert_eq!(kept.back().unwrap().at, 100 + ACTIVITY_KEPT as u64 - 1);
    assert!(!m.state.activity.contains_key("bob"));
    assert!(!m.state.activity.contains_key("carol"));
}

/// An account is created only by its own new passkey, over the whole
/// account and the session its sign-up opens; never with a refresh token
/// for another account.
#[test]
fn an_account_is_created_by_its_own_passkey() {
    let (mut m, _alice, mut bob, _carol) = proven_cluster();
    let mut dave = Soft::new("localhost", "http://localhost:3000");
    let SystemCmd::CreateAccount { account, .. } = account_with("dave", &mut dave) else {
        unreachable!()
    };
    let (_token, hash, record) = refresh_token("dave", &dave.credential_id(), "s1");
    let grant = super::RefreshGrant { hash, record };
    let create = |refresh: Option<super::RefreshGrant>, consent| SystemCmd::CreateAccount {
        at: 0,
        account: account.clone(),
        invite: None,
        refresh,
        consent,
    };
    let args = super::account_args(&account, Some(&grant));
    let mut elsewhere = account.clone();
    elsewhere.approved_releases = ["9.9.9".to_owned()].into_iter().collect();
    let (_t, h, alices) = refresh_token("alice", &dave.credential_id(), "s1");
    let for_alice = super::RefreshGrant {
        hash: h,
        record: alices,
    };
    for (why, cmd) in [
        (
            "no consent",
            create(Some(grant.clone()), serde_json::Value::Null),
        ),
        (
            "another passkey's",
            create(
                Some(grant.clone()),
                consent(&mut bob, "account:dave", "__create_account", &args),
            ),
        ),
        (
            "over another release",
            create(
                Some(grant.clone()),
                consent(
                    &mut dave,
                    "account:dave",
                    "__create_account",
                    &super::account_args(&elsewhere, Some(&grant)),
                ),
            ),
        ),
        (
            "a refresh token for alice",
            create(
                Some(for_alice.clone()),
                consent(
                    &mut dave,
                    "account:dave",
                    "__create_account",
                    &super::account_args(&account, Some(&for_alice)),
                ),
            ),
        ),
    ] {
        assert!(m.run(cmd).is_err(), "{why}");
    }
    assert!(!m.state.accounts.contains_key("dave"));
    let real = consent(&mut dave, "account:dave", "__create_account", &args);
    let _ok = m.run(create(Some(grant.clone()), real)).unwrap();
    assert!(m.state.accounts.contains_key("dave"));
    assert_eq!(m.state.refresh[&grant.hash].account, "dave");
}

/// An API key is revoked only with its account's passkey consent over
/// that key, once.
#[test]
fn an_api_key_is_revoked_with_its_accounts_passkey() {
    let (mut m, mut alice, mut bob, _carol) = proven_cluster();
    let record: crate::types::ApiKeyRecord = serde_json::from_value(json!({
        "keyId": "k1", "account": "alice", "label": "ci", "apps": null,
        "methods": null, "readOnly": false, "allowConfirm": false, "expiresAt": 99,
    }))
    .unwrap();
    let _prev = m.state.api_keys.insert("h1".into(), record.clone());
    let _prev = m.state.api_keys.insert(
        "h2".into(),
        crate::types::ApiKeyRecord {
            key_id: "k2".into(),
            ..record
        },
    );
    let revoke = |consent| SystemCmd::RevokeApiKey {
        key_id: "k1".into(),
        actor: "alice".into(),
        consent,
    };
    let args = json!({ "keyId": "k1" });
    let c = consent(&mut bob, "account:alice", "__revoke_api_key", &args);
    assert!(m.run(revoke(c)).is_err(), "bob's passkey");
    needs_consent(
        &mut m,
        &mut alice,
        ("account:alice", "__revoke_api_key"),
        &args,
        &json!({ "keyId": "k2" }),
        revoke,
    );
    assert!(!m.state.api_keys.contains_key("h1"));
    assert!(m.state.api_keys.contains_key("h2"));
}

/// A passkey's counter moves only to what an assertion of that passkey
/// carries.
#[test]
fn a_counter_moves_with_the_assertion_that_moved_it() {
    let (mut m, mut alice, mut bob, _carol) = proven_cluster();
    let id = alice.credential_id();
    let set = |count, assertion: &calimero_identity::Assertion| SystemCmd::SetSignCount {
        credential_id: id.clone(),
        count,
        assertion: serde_json::to_value(assertion).unwrap(),
    };
    let theirs = calimero_identity::Assertion {
        credential_id: id.clone(),
        ..bob.assert(&[1; 32])
    };
    let a = alice.assert(&[1; 32]);
    let carried = calimero_identity::assertion_counter(
        &m.state.account_of_credential(&id).unwrap().1.clone(),
        &a,
    )
    .unwrap();
    assert!(m
        .run(SystemCmd::SetSignCount {
            credential_id: id.clone(),
            count: u32::MAX,
            assertion: serde_json::Value::Null,
        })
        .is_err());
    assert!(m.run(set(u32::MAX, &a)).is_err(), "past what it carries");
    assert!(m.run(set(carried, &theirs)).is_err(), "bob's signature");
    let _ok = m.run(set(carried, &a)).unwrap();
    assert_eq!(m.state.accounts["alice"].passkeys[0].sign_count, carried);
    assert!(m.run(set(carried, &a)).is_err(), "replayed");
}

/// An organization is renamed by its owner or an admin with their
/// passkey consent over exactly the new name, once; a member below
/// admin and someone outside it are refused, and so is a name that
/// would not make an organization.
#[test]
fn an_organization_is_renamed_with_an_admins_passkey() {
    let (mut m, mut alice, mut bob, mut carol) = proven_cluster();
    let rename = |actor: &str, name: &str, consent: Value| SystemCmd::RenameOrg {
        org: "org".into(),
        name: name.into(),
        actor: actor.into(),
        consent,
    };
    needs_consent(
        &mut m,
        &mut alice,
        ("org:org", "__rename_org"),
        &json!({ "name": "Docs Labs" }),
        &json!({ "name": "Other" }),
        |c| rename("alice", "Docs Labs", c),
    );
    assert_eq!(m.state.orgs["org"].name, "Docs Labs");
    for (actor, pk) in [("bob", &mut bob), ("carol", &mut carol)] {
        let c = consent(pk, "org:org", "__rename_org", &json!({ "name": "Mine" }));
        assert_eq!(
            m.run(rename(actor, "Mine", c)).unwrap_err(),
            super::ONLY_ADMINS_RENAME,
            "{actor}"
        );
    }
    let _prev = m
        .state
        .orgs
        .get_mut("org")
        .unwrap()
        .members
        .insert("bob".into(), OrgRole::Admin);
    for bad in [
        String::new(),
        " Docs".to_owned(),
        "Do\ncs".to_owned(),
        "x".repeat(super::MAX_ORG_NAME_CHARS + 1),
    ] {
        assert!(
            m.run_unproven(rename("bob", &bad, Value::Null)).is_err(),
            "{bad:?}"
        );
    }
    let long = "y".repeat(super::MAX_ORG_NAME_CHARS);
    let c = consent(
        &mut bob,
        "org:org",
        "__rename_org",
        &json!({ "name": long }),
    );
    let out = m.run(rename("bob", &long, c)).unwrap();
    assert_eq!(out, json!({ "orgId": "org", "name": long }));
    assert_eq!(m.state.orgs["org"].name, long);
}

/// A `CreateOrg` an earlier release logged (no `name_fits`) replays
/// taking any name, as that release did; one the API logs now refuses a
/// name that does not fit, and the flag is left out when false, so an
/// entry from before writes as it did.
#[test]
fn a_create_org_entry_from_before_takes_any_name() {
    let (mut m, _alice, _bob, _carol) = proven_cluster();
    let cmd: SystemCmd = serde_json::from_value(json!({
        "op": "createOrg", "org_id": "o1", "name": " padded ", "actor": "alice"
    }))
    .unwrap();
    let back = serde_json::to_value(&cmd).unwrap();
    assert!(back.get("name_fits").is_none(), "{back}");
    let _ok = m.run_unproven(cmd).unwrap();
    assert_eq!(m.state.orgs["o1"].name, " padded ");
    let create = |id: &str, name: &str| SystemCmd::CreateOrg {
        org_id: id.into(),
        name: name.into(),
        actor: "alice".into(),
        release: None,
        recovery_key: None,
        at: 0,
        name_fits: true,
    };
    assert!(m.run_unproven(create("o2", " padded ")).is_err());
    assert!(!m.state.orgs.contains_key("o2"));
    let _ok = m.run_unproven(create("o2", "Fits")).unwrap();
    assert_eq!(m.state.orgs["o2"].name, "Fits");
}

/// Membership changes apply only with the actor's passkey consent over
/// exactly that change, once; a member leaving consents too.
#[test]
fn memberships_need_the_actors_passkey() {
    let (mut m, mut alice, mut bob, _carol) = proven_cluster();
    needs_consent(
        &mut m,
        &mut alice,
        ("org:org", "__set_org_role"),
        &json!({ "account": "bob", "role": "admin" }),
        &json!({ "account": "bob", "role": "member" }),
        |consent| SystemCmd::SetOrgRole {
            org: "org".into(),
            account: "bob".into(),
            role: OrgRole::Admin,
            actor: "alice".into(),
            consent,
        },
    );
    assert_eq!(m.state.orgs["org"].members["bob"], OrgRole::Admin);
    needs_consent(
        &mut m,
        &mut alice,
        ("org:org", "__set_team_member"),
        &json!({ "team": "t", "account": "bob", "lead": false, "role": null, "capabilities": null }),
        &json!({ "team": "t", "account": "bob", "lead": true, "role": null, "capabilities": null }),
        |consent| SystemCmd::SetTeamMember {
            org: "org".into(),
            team: "t".into(),
            account: "bob".into(),
            lead: false,
            role: None,
            capabilities: None,
            actor: "alice".into(),
            consent,
        },
    );
    assert!(m.state.orgs["org"].in_team("bob", "t"));
    needs_consent(
        &mut m,
        &mut alice,
        ("org:org", "__set_capabilities"),
        &json!({ "team": "t", "account": "bob", "capabilities": 3 }),
        &json!({ "team": null, "account": "bob", "capabilities": 3 }),
        |consent| SystemCmd::SetCapabilities {
            org: "org".into(),
            team: Some("t".into()),
            account: "bob".into(),
            capabilities: 3,
            actor: "alice".into(),
            consent,
        },
    );
    needs_consent(
        &mut m,
        &mut alice,
        ("org:org", "__set_default_capabilities"),
        &json!({ "capabilities": 5 }),
        &json!({ "capabilities": 7 }),
        |consent| SystemCmd::SetDefaultCapabilities {
            org: "org".into(),
            team: None,
            capabilities: 5,
            actor: "alice".into(),
            consent,
        },
    );
    assert_eq!(m.state.orgs["org"].default_capabilities, 5);
    needs_consent(
        &mut m,
        &mut alice,
        ("org:org", "__remove_team_member"),
        &json!({ "team": "t", "account": "bob" }),
        &json!({ "team": "t", "account": "alice" }),
        |consent| SystemCmd::RemoveTeamMember {
            org: "org".into(),
            team: "t".into(),
            account: "bob".into(),
            actor: "alice".into(),
            consent,
            subtree: true,
        },
    );
    // Leaving takes the leaver's own consent, not the owner's.
    let leave = |consent| SystemCmd::RemoveOrgMember {
        org: "org".into(),
        account: "bob".into(),
        actor: "bob".into(),
        consent,
    };
    let args = json!({ "account": "bob" });
    let c = consent(&mut alice, "org:org", "__remove_member", &args);
    assert!(m.run(leave(c)).is_err());
    needs_consent(
        &mut m,
        &mut bob,
        ("org:org", "__remove_member"),
        &args,
        &json!({ "account": "alice" }),
        leave,
    );
    assert!(!m.state.orgs["org"].members.contains_key("bob"));
}

/// An invite is minted with the actor's consent over its key and terms,
/// and redeemed only with a signature by the token's key, for the
/// account it names, once: a node that saw only the log cannot.
#[test]
fn invites_are_redeemed_with_the_tokens_key() {
    let (mut m, mut alice, _bob, _carol) = proven_cluster();
    let token = "inv_secret";
    let key = crate::proof::invite_key(token);
    let invite: crate::types::Invite = serde_json::from_value(json!({
        "org": "org", "role": "member", "teams": [], "expiresAt": 100, "usesLeft": 2,
    }))
    .unwrap();
    let args = super::invite_args(&key, &invite);
    let mut admin = args.clone();
    admin["role"] = json!("admin");
    needs_consent(
        &mut m,
        &mut alice,
        ("org:org", "__create_invite"),
        &args,
        &admin,
        |consent| SystemCmd::CreateInvite {
            invite_key: key.clone(),
            invite: invite.clone(),
            actor: "alice".into(),
            consent,
            teams_seen: true,
        },
    );
    let redeem = |signature: String| SystemCmd::RedeemInvite {
        invite_key: key.clone(),
        account: "carol".into(),
        now: 10,
        signature,
        free_when_unchanged: true,
    };
    assert!(m.run(redeem(String::new())).is_err(), "no signature");
    let forged = crate::proof::sign_invite_redeem("inv_guess", "carol", 10);
    assert!(m.run(redeem(forged)).is_err(), "another token's key");
    let for_bob = crate::proof::sign_invite_redeem(token, "bob", 10);
    assert!(m.run(redeem(for_bob)).is_err(), "another account");
    let earlier = crate::proof::sign_invite_redeem(token, "carol", 9);
    assert!(m.run(redeem(earlier)).is_err(), "another time");
    assert!(!m.state.orgs["org"].members.contains_key("carol"));
    let real = crate::proof::sign_invite_redeem(token, "carol", 10);
    let _ok = m.run(redeem(real.clone())).unwrap();
    assert!(m.state.orgs["org"].members.contains_key("carol"));
    assert!(m.run(redeem(real)).is_err(), "replay");
    assert_eq!(m.state.invites[&key].uses_left, 1);
}

/// An invite of `org` kept under `token`'s key: `role`, `teams`, `uses`.
fn keep_invite(m: &mut SystemMachine, token: &str, role: &str, teams: &[&str], uses: u32) {
    let invite: crate::types::Invite = serde_json::from_value(json!({
        "org": "org", "role": role, "teams": teams, "expiresAt": 100, "usesLeft": uses,
    }))
    .unwrap();
    let _prev = m
        .state
        .invites
        .insert(crate::proof::invite_key(token), invite);
}

/// `account`'s redemption of `token` at `now`, as this release's API logs it.
fn redemption(token: &str, account: &str, now: u64) -> SystemCmd {
    SystemCmd::RedeemInvite {
        invite_key: crate::proof::invite_key(token),
        account: account.into(),
        now,
        signature: crate::proof::sign_invite_redeem(token, account, now),
        free_when_unchanged: true,
    }
}

/// The owner opening their own one-use link changes nothing for them, so
/// it spends nothing: the person it was made for still joins with it, and
/// only that redemption removes it. Before, the owner's spent the only
/// use and the invitee was told the invite was unknown.
#[test]
fn a_member_redeeming_an_invite_leaves_its_use_for_the_invitee() {
    let (mut m, _alice, _bob, _carol) = proven_cluster();
    keep_invite(&mut m, "inv_link", "member", &[], 1);
    let key = crate::proof::invite_key("inv_link");
    let out = m.run(redemption("inv_link", "alice", 10)).unwrap();
    assert_eq!(out, json!({ "org": "org" }));
    assert_eq!(m.state.orgs["org"].members["alice"], OrgRole::Owner);
    assert_eq!(m.state.invites[&key].uses_left, 1, "the owner spent none");
    // A member at the invite's role spends none either.
    let _same = m.run(redemption("inv_link", "bob", 11)).unwrap();
    assert_eq!(m.state.invites[&key].uses_left, 1, "bob spent none");
    let out = m.run(redemption("inv_link", "carol", 12)).unwrap();
    assert_eq!(out, json!({ "org": "org" }));
    assert_eq!(m.state.orgs["org"].members["carol"], OrgRole::Member);
    assert!(
        !m.state.invites.contains_key(&key),
        "the invitee used it up"
    );
    let e = m.run(redemption("inv_link", "carol", 13)).unwrap_err();
    assert_eq!(e, INVITE_GONE);
}

/// A member spends a use when the invite gives them something: a higher
/// role, or a team they are not in yet.
#[test]
fn a_member_spends_a_use_when_the_invite_raises_or_adds() {
    let (mut m, _alice, _bob, _carol) = proven_cluster();
    keep_invite(&mut m, "inv_admin", "admin", &[], 2);
    let _ok = m.run(redemption("inv_admin", "bob", 10)).unwrap();
    assert_eq!(m.state.orgs["org"].members["bob"], OrgRole::Admin);
    let admin = crate::proof::invite_key("inv_admin");
    assert_eq!(m.state.invites[&admin].uses_left, 1, "a raised role");
    keep_invite(&mut m, "inv_team", "member", &["t"], 2);
    let _ok = m.run(redemption("inv_team", "bob", 11)).unwrap();
    assert!(m.state.orgs["org"].teams["t"].members.contains_key("bob"));
    let team = crate::proof::invite_key("inv_team");
    assert_eq!(m.state.invites[&team].uses_left, 1, "an added team");
    // In the team now, and an admin: the same invite gives bob nothing.
    let _same = m.run(redemption("inv_team", "bob", 12)).unwrap();
    assert_eq!(m.state.invites[&team].uses_left, 1, "nothing new");
}

/// A redemption an earlier release logged (no `free_when_unchanged`)
/// replays as that release applied it: the owner's spends the only use
/// and removes the invite, so the invitee's later redemption is refused
/// on a replica that replays the log after a restart as on one that
/// applied it then.
#[test]
fn a_redemption_logged_before_free_redemptions_replays_as_it_did() {
    let (mut m, _alice, _bob, _carol) = proven_cluster();
    keep_invite(&mut m, "inv_link", "member", &[], 1);
    let key = crate::proof::invite_key("inv_link");
    // The entry as rc.10 logged it: no flag.
    let logged = json!({
        "op": "redeemInvite",
        "invite_key": &key,
        "account": "alice",
        "now": 10,
        "signature": crate::proof::sign_invite_redeem("inv_link", "alice", 10),
    });
    let old: SystemCmd = serde_json::from_value(logged).unwrap();
    let out = m.run(old).unwrap();
    assert_eq!(out, json!({ "org": "org" }));
    assert!(!m.state.invites.contains_key(&key), "spent, as it was");
    let e = m.run(redemption("inv_link", "carol", 11)).unwrap_err();
    assert_eq!(e, INVITE_GONE);
    assert!(!m.state.orgs["org"].members.contains_key("carol"));
    // This release logs the flag, so its entries replay the new rule.
    let new = serde_json::to_value(redemption("inv_link", "alice", 10)).unwrap();
    assert_eq!(new["free_when_unchanged"], json!(true), "{new}");
}

// ------------------------------------------- teams and leaving members

use super::DELETE_TEAM_NEEDS_CONSENT;
use crate::types::{AppRecord, Audience};

/// `proven_cluster` plus a restricted team `c` under `t`, bob with a
/// direct row in both, and an app `inapp` on `c`.
fn nested_teams() -> (SystemMachine, Soft, Soft) {
    let (mut m, alice, bob, _carol) = proven_cluster();
    let o = m.state.orgs.get_mut("org").unwrap();
    let _prev = o.teams.insert(
        "c".into(),
        serde_json::from_value(json!({
            "teamId": "c", "name": "C", "parent": "t", "join": "invite", "members": {}
        }))
        .unwrap(),
    );
    for team in ["t", "c"] {
        let _prev = o
            .teams
            .get_mut(team)
            .unwrap()
            .members
            .insert("bob".into(), false);
    }
    let _prev = m.state.apps.insert(
        "inapp".into(),
        serde_json::from_value(json!({
            "appId": "inapp", "name": "inapp", "group": 3, "bundle": "b1",
            "owner": "alice", "org": "org", "access": { "kind": "team", "team": "c" },
            "replicas": [1, 2, 3], "createdAt": 0
        }))
        .unwrap(),
    );
    (m, alice, bob)
}

/// A removal from a team takes the direct rows below it too.
#[test]
fn a_team_removal_takes_the_rows_below_it() {
    let (mut m, mut alice, _bob) = nested_teams();
    let c = consent(
        &mut alice,
        "org:org",
        "__remove_team_member",
        &json!({ "team": "t", "account": "bob" }),
    );
    let _ok = m
        .run(SystemCmd::RemoveTeamMember {
            org: "org".into(),
            team: "t".into(),
            account: "bob".into(),
            actor: "alice".into(),
            consent: c,
            subtree: true,
        })
        .unwrap();
    let o = &m.state.orgs["org"];
    assert!(!o.in_team("bob", "t") && !o.in_team("bob", "c"));
    assert!(!m.state.may_reach(&m.state.apps["inapp"], Some("bob")));
}

/// Deleting a team that holds apps needs the deleter's passkey over
/// exactly those apps; one without apps goes without it.
#[test]
fn deleting_a_team_with_apps_needs_consent_over_them() {
    let (mut m, mut alice, _bob) = nested_teams();
    let delete = |team: &str, consent| SystemCmd::DeleteTeam {
        org: "org".into(),
        team: team.into(),
        actor: "alice".into(),
        consent,
        apps_need_consent: true,
    };
    assert_eq!(
        m.run(delete("t", serde_json::Value::Null)).unwrap_err(),
        DELETE_TEAM_NEEDS_CONSENT
    );
    let fewer = consent(
        &mut alice,
        "org:org",
        "__delete_team",
        &json!({ "team": "t", "apps": [] }),
    );
    assert!(m.run(delete("t", fewer)).is_err(), "another list of apps");
    let real = consent(
        &mut alice,
        "org:org",
        "__delete_team",
        &json!({ "team": "t", "apps": ["inapp"] }),
    );
    let out = m.run(delete("t", real)).unwrap();
    assert_eq!(out["apps"], json!(["inapp"]));
    assert!(!m.state.apps.contains_key("inapp"));

    let (mut m, _alice, _bob) = nested_teams();
    let _gone = m.state.apps.remove("inapp");
    let out = m.run(delete("c", serde_json::Value::Null)).unwrap();
    assert_eq!(out["apps"], json!([]));
}

/// Below admin, an invite names only teams the inviter sees, whatever
/// INVITE_MEMBERS they hold organization-wide.
#[test]
fn an_invite_names_only_teams_the_inviter_sees() {
    let (mut m, _alice, mut bob) = nested_teams();
    let o = m.state.orgs.get_mut("org").unwrap();
    let _prev = o.teams.insert(
        "x".into(),
        serde_json::from_value(json!({
            "teamId": "x", "name": "X", "join": "invite", "members": {}
        }))
        .unwrap(),
    );
    let _prev = o
        .capabilities
        .insert("bob".into(), crate::types::caps::INVITE_MEMBERS);
    let mint = |m: &mut SystemMachine, bob: &mut Soft, token: &str, team: &str| {
        let key = crate::proof::invite_key(token);
        let invite: crate::types::Invite = serde_json::from_value(json!({
            "org": "org", "role": "member", "teams": [team], "expiresAt": 100, "usesLeft": 1,
        }))
        .unwrap();
        let c = consent(
            bob,
            "org:org",
            "__create_invite",
            &super::invite_args(&key, &invite),
        );
        m.run(SystemCmd::CreateInvite {
            invite_key: key,
            invite,
            actor: "bob".into(),
            consent: c,
            teams_seen: true,
        })
    };
    assert!(
        mint(&mut m, &mut bob, "inv_t", "t").is_ok(),
        "a team he is in"
    );
    assert_eq!(
        mint(&mut m, &mut bob, "inv_x", "x").unwrap_err(),
        "an invite names only teams the inviter sees"
    );
}

/// Commands an earlier release logged, without this release's flags,
/// replay as that release applied them: a team removal takes one row, an
/// invite skips the sight check, a team deletion needs no passkey. The
/// entries this release logs carry the flags.
#[test]
fn team_commands_logged_before_these_rules_replay_as_they_did() {
    let (mut m, mut alice, mut bob) = nested_teams();
    let c = consent(
        &mut alice,
        "org:org",
        "__remove_team_member",
        &json!({ "team": "t", "account": "bob" }),
    );
    let old: SystemCmd = serde_json::from_value(json!({
        "op": "removeTeamMember", "org": "org", "team": "t", "account": "bob",
        "actor": "alice", "consent": c,
    }))
    .unwrap();
    let _ok = m.run(old).unwrap();
    assert!(
        m.state.orgs["org"].in_team("bob", "c"),
        "the row below stays"
    );

    let _prev = m
        .state
        .orgs
        .get_mut("org")
        .unwrap()
        .capabilities
        .insert("bob".into(), crate::types::caps::INVITE_MEMBERS);
    let key = crate::proof::invite_key("inv_old");
    let invite: crate::types::Invite = serde_json::from_value(json!({
        "org": "org", "role": "member", "teams": ["t"], "expiresAt": 100, "usesLeft": 1,
    }))
    .unwrap();
    let c = consent(
        &mut bob,
        "org:org",
        "__create_invite",
        &super::invite_args(&key, &invite),
    );
    let old: SystemCmd = serde_json::from_value(json!({
        "op": "createInvite", "invite_key": key, "invite": invite, "actor": "bob",
        "consent": c,
    }))
    .unwrap();
    let _ok = m.run(old).unwrap();
    assert!(m.state.invites.contains_key(&key), "minted as it was");

    let old: SystemCmd = serde_json::from_value(json!({
        "op": "deleteTeam", "org": "org", "team": "t", "actor": "alice",
    }))
    .unwrap();
    let out = m.run(old).unwrap();
    assert_eq!(out["apps"], json!(["inapp"]), "deleted without a passkey");

    let new = |cmd: SystemCmd| serde_json::to_value(cmd).unwrap();
    let removal = new(SystemCmd::RemoveTeamMember {
        org: "org".into(),
        team: "t".into(),
        account: "bob".into(),
        actor: "alice".into(),
        consent: serde_json::Value::Null,
        subtree: true,
    });
    assert_eq!(removal["subtree"], json!(true), "{removal}");
    let deletion = new(SystemCmd::DeleteTeam {
        org: "org".into(),
        team: "t".into(),
        actor: "alice".into(),
        consent: serde_json::Value::Null,
        apps_need_consent: true,
    });
    assert_eq!(deletion["apps_need_consent"], json!(true), "{deletion}");
}

/// An account an organization's app names (its owner, an `Accounts`
/// list) reaches it, and receives its events, only while a member.
#[test]
fn an_org_app_is_reached_only_by_current_members() {
    let (mut m, _alice, _bob, _carol) = proven_cluster();
    let listed: AppRecord = serde_json::from_value(json!({
        "appId": "dm", "name": "dm", "group": 4, "bundle": "b1", "owner": "bob",
        "org": "org", "access": { "kind": "accounts", "accounts": ["alice", "bob", "carol"] },
        "replicas": [1, 2, 3], "createdAt": 0
    }))
    .unwrap();
    let s = &m.state;
    assert!(s.may_reach(&listed, Some("alice")) && s.may_reach(&listed, Some("bob")));
    assert!(
        !s.may_reach(&listed, Some("carol")),
        "listed, never a member"
    );
    let _gone = m.state.orgs.get_mut("org").unwrap().members.remove("bob");
    let s = &m.state;
    assert!(!s.may_reach(&listed, Some("bob")), "listed owner, removed");
    assert!(!s.receives(&listed, &Audience::All, Some("alice"), Some("bob")));
    assert!(s.receives(&listed, &Audience::All, Some("bob"), Some("alice")));
    // Outside an organization, the list alone decides.
    let personal = AppRecord {
        org: None,
        ..listed
    };
    assert!(m.state.may_reach(&personal, Some("carol")));
}

/// An `Accounts` instance (a DM, a private channel) of a package with an
/// app team in its organization is reached only by those who reach the
/// app team, its owner too: a person out of Chat is out of its DMs. An
/// instance of a package with no app team there keeps its list alone. A
/// `ScopeApps` an earlier release logged (no `accounts_in_app_team`)
/// finds who reaches such an instance as that release did.
#[test]
fn an_accounts_instance_is_reached_only_through_its_app_team() {
    let dm = |id: &str, bundle: &str| -> AppRecord {
        serde_json::from_value(json!({
            "appId": id, "name": id, "group": 7, "bundle": bundle, "owner": "carol",
            "org": "org",
            "access": { "kind": "accounts", "accounts": ["alice", "bob", "carol", "dave"] },
            "replicas": [1, 2, 3], "createdAt": 0
        }))
        .unwrap()
    };
    let fresh = || {
        let (mut m, _alice, _bob, _carol) = proven_cluster();
        chat_org(&mut m);
        let _prev = m.state.apps.insert("dm".into(), dm("dm", "bc"));
        m
    };
    let m = fresh();
    let chat = m.state.apps["dm"].clone();
    let s = &m.state;
    assert!(s.may_reach(&chat, Some("alice")) && s.may_reach(&chat, Some("bob")));
    assert!(!s.may_reach(&chat, Some("carol")), "the owner, out of Chat");
    assert!(!s.may_reach(&chat, Some("dave")), "listed, out of Chat");
    assert!(!s.receives(&chat, &Audience::All, Some("bob"), Some("dave")));
    assert!(s.may_reach_as_logged(&chat, Some("dave"), true, false));
    // No app team for its package: the list alone, as before.
    let raw = dm("raw", "b1");
    assert!(s.may_reach(&raw, Some("carol")) && s.may_reach(&raw, Some("dave")));

    // Moving the DM onto Chat's team gives rows to who reaches it: now
    // Alice and Bob; as logged before, Carol and Dave too.
    let scope = |flag: bool| SystemCmd::ScopeApps {
        org: "org".into(),
        apps: Some(vec![super::ScopeApp {
            package: "com.chat".into(),
            name: None,
            instances: Some(vec!["dm".into()]),
            join: None,
            keep: None,
            child_teams: None,
        }]),
        shared: None,
        nonce: "n".into(),
        actor: "alice".into(),
        consent: Value::Null,
        owner_in_team: true,
        accounts_in_app_team: flag,
        member_child_teams: true,
        all_apps_rows: true,
        by_release: true,
    };
    let rows = |m: &SystemMachine| -> Vec<String> {
        m.state.orgs["org"].teams["chat"]
            .members
            .keys()
            .cloned()
            .collect()
    };
    for (flag, want) in [
        (true, &["alice", "bob"][..]),
        (false, &["alice", "bob", "carol", "dave"][..]),
    ] {
        let mut m = fresh();
        let _ok = m.run_authorized(scope(flag)).unwrap();
        assert_eq!(rows(&m), want, "{flag}");
    }
}

/// Two packages ship one component (`bc`): a `ScopeApps` puts each
/// instance in the app team of the package its release names. One an
/// earlier release logged (no `by_release`) replays as that release
/// judged it, by the bundle's record (Chat's): both instances on Chat's
/// team, Notes' made empty.
#[test]
fn scoping_judges_an_instance_by_its_release_and_old_entries_by_the_bundle() {
    let instance = |id: &str, package: &str| -> AppRecord {
        serde_json::from_value(json!({
            "appId": id, "name": id, "group": 7, "bundle": "bc", "owner": "alice",
            "org": "org", "access": { "kind": "org" }, "replicas": [1, 2, 3],
            "createdAt": 0, "release": { "package": package, "version": "1.0.0" }
        }))
        .unwrap()
    };
    let fresh = || {
        let (mut m, _alice, _bob, _carol) = proven_cluster();
        chat_org(&mut m);
        let mut notes = m.state.packages["com.chat"].clone();
        "com.notes".clone_into(&mut notes.name);
        let _prev = m.state.packages.insert("com.notes".into(), notes);
        for (id, package) in [("c1", "com.chat"), ("n1", "com.notes")] {
            let _prev = m.state.apps.insert(id.into(), instance(id, package));
        }
        m
    };
    let scope = |flag: bool| SystemCmd::ScopeApps {
        org: "org".into(),
        apps: Some(
            ["com.chat", "com.notes"]
                .map(|package| super::ScopeApp {
                    package: package.into(),
                    name: None,
                    instances: None,
                    join: None,
                    keep: None,
                    child_teams: None,
                })
                .to_vec(),
        ),
        shared: None,
        nonce: "n".into(),
        actor: "alice".into(),
        consent: Value::Null,
        owner_in_team: true,
        accounts_in_app_team: true,
        member_child_teams: true,
        all_apps_rows: true,
        by_release: flag,
    };
    // The team each instance is on after the step.
    let teams = |m: &SystemMachine| -> Vec<Option<String>> {
        ["c1", "n1"]
            .map(|id| {
                let team = match &m.state.apps[id].access {
                    crate::types::Access::Team { team } => team,
                    _ => return None,
                };
                m.state.orgs["org"].teams[team]
                    .app_package()
                    .map(str::to_owned)
            })
            .to_vec()
    };
    let mut m = fresh();
    let _ok = m.run_authorized(scope(true)).unwrap();
    assert_eq!(
        teams(&m),
        [Some("com.chat".to_owned()), Some("com.notes".to_owned())]
    );
    let mut m = fresh();
    let _ok = m.run_authorized(scope(false)).unwrap();
    assert_eq!(
        teams(&m),
        [Some("com.chat".to_owned()), Some("com.chat".to_owned())]
    );
    // Such an entry reads without the flag and writes as it was logged.
    let old = serde_json::to_value(scope(false)).unwrap();
    assert!(old.get("by_release").is_none(), "{old}");
    let read: SystemCmd = serde_json::from_value(old.clone()).unwrap();
    assert_eq!(serde_json::to_value(&read).unwrap(), old);
    assert_eq!(
        serde_json::to_value(scope(true)).unwrap()["by_release"],
        json!(true)
    );
}

/// An app team made before its default held `CREATE_CHILD_TEAM` gets it
/// only when an admin says so (`ScopeApps` with a package's
/// `childTeams`), never on replay or as a side effect: a step without it
/// leaves the default and the rows as they are. Naming it decides that
/// team alone, as `join` does, so another open app team stays open.
#[test]
fn an_older_app_team_gets_child_teams_when_an_admin_turns_them_on() {
    use crate::types::caps;

    let (mut m, _alice, _bob, _carol) = proven_cluster();
    chat_org(&mut m);
    let o = m.state.orgs.get_mut("org").unwrap();
    // Chat's team as an earlier release made it: inviting, no channels;
    // Carol's row, added one by one, holds what it got then.
    let chat = o.teams.get_mut("chat").unwrap();
    chat.default_capabilities = Some(caps::INVITE_MEMBERS);
    let _prev = chat.members.insert("carol".into(), false);
    let _prev = chat
        .capabilities
        .insert("carol".into(), caps::INVITE_MEMBERS);
    let _prev = o.teams.insert(
        "blocks".into(),
        serde_json::from_value(json!({
            "teamId": "blocks", "name": "Blocks", "join": "everyone",
            "members": { "alice": true },
            "metadata": { "app": { "package": "com.blocks" } }
        }))
        .unwrap(),
    );
    let scope = |child_teams: Option<bool>| SystemCmd::ScopeApps {
        org: "org".into(),
        apps: Some(vec![super::ScopeApp {
            package: "com.chat".into(),
            name: None,
            instances: Some(vec![]),
            join: None,
            keep: None,
            child_teams,
        }]),
        // Absent, a step without `childTeams` would close every app
        // team here (there are two): this one keeps Blocks open.
        shared: child_teams.is_none().then_some(false),
        nonce: "n".into(),
        actor: "alice".into(),
        consent: Value::Null,
        owner_in_team: true,
        accounts_in_app_team: true,
        member_child_teams: true,
        all_apps_rows: true,
        by_release: true,
    };
    let channel = |id: &str, who: &str| SystemCmd::CreateTeam {
        org: "org".into(),
        team_id: id.into(),
        name: id.into(),
        parent: Some("chat".into()),
        join: TeamJoin::Invite,
        actor: who.into(),
        member_child_teams: true,
    };
    let default = |m: &SystemMachine| m.state.orgs["org"].teams["chat"].default_capabilities;
    let refused = "not allowed to create this team";

    // Without it (as every entry before it): the default stays, and
    // Bob and Carol, in Chat, make no channel.
    let _ok = m.run_authorized(scope(None)).unwrap();
    assert_eq!(default(&m), Some(caps::INVITE_MEMBERS));
    for who in ["bob", "carol"] {
        assert_eq!(m.run_authorized(channel(who, who)), Err(refused));
    }

    // Turned on, the step names Chat alone: both make one (Carol by her
    // own row too), and Blocks stays open.
    let _ok = m.run_authorized(scope(Some(true))).unwrap();
    let both = caps::INVITE_MEMBERS | caps::CREATE_CHILD_TEAM;
    assert_eq!(default(&m), Some(both));
    let o = &m.state.orgs["org"];
    assert_eq!(o.teams["chat"].capabilities["carol"], both);
    assert_eq!(o.teams["blocks"].join, TeamJoin::Everyone);
    for who in ["bob", "carol"] {
        let id = format!("{who}1");
        let _ok = m.run_authorized(channel(&id, who)).unwrap();
    }

    // Turned off again: no more.
    let _ok = m.run_authorized(scope(Some(false))).unwrap();
    assert_eq!(default(&m), Some(caps::INVITE_MEMBERS));
    for who in ["bob", "carol"] {
        let id = format!("{who}2");
        assert_eq!(m.run_authorized(channel(&id, who)), Err(refused));
    }

    // An earlier release's entry carries no `childTeams`, and one
    // without it is logged as before.
    let logged: super::ScopeApp = serde_json::from_value(json!({ "package": "com.chat" })).unwrap();
    assert_eq!(logged.child_teams, None);
    assert_eq!(
        serde_json::to_value(&logged).unwrap(),
        json!({ "package": "com.chat" })
    );
}

/// A standing grant of every app added later is the owner's or an
/// admin's: set on a member with their passkey consent over exactly
/// that, once, and minted on an invite only by them. Redeeming such an
/// invite gives it (and is a change, so it spends a use even for a
/// member at the invite's role); removal from the organization ends it.
/// An invite without it is consented to in the bytes it was before.
#[test]
fn every_app_added_later_is_an_admins_to_give() {
    let (mut m, mut alice, _bob, _carol) = proven_cluster();
    let set = |actor: &str, account: &str, on: bool, consent: Value| SystemCmd::SetAllApps {
        org: "org".into(),
        account: account.into(),
        all_apps: on,
        actor: actor.into(),
        consent,
    };
    needs_consent(
        &mut m,
        &mut alice,
        ("org:org", "__set_all_apps"),
        &super::all_apps_args("bob", true),
        &super::all_apps_args("bob", false),
        |c| set("alice", "bob", true, c),
    );
    assert!(m.state.orgs["org"].all_apps.contains("bob"));
    assert_eq!(
        m.run_unproven(set("bob", "bob", false, Value::Null)),
        Err(super::ONLY_ADMINS_ALL_APPS.to_owned())
    );
    assert!(m
        .run_unproven(set("alice", "carol", true, Value::Null))
        .is_err());
    let _ok = m
        .run_unproven(set("alice", "bob", false, Value::Null))
        .unwrap();
    assert!(m.state.orgs["org"].all_apps.is_empty());

    // Invites: only an admin's carries it, and only then is it named.
    let invite = |all_apps: bool| -> crate::types::Invite {
        serde_json::from_value(json!({
            "org": "org", "role": "member", "expiresAt": 100, "usesLeft": 2,
            "allApps": all_apps,
        }))
        .unwrap()
    };
    assert!(super::invite_args("k", &invite(false))
        .get("allApps")
        .is_none());
    assert_eq!(super::invite_args("k", &invite(true))["allApps"], true);
    let create = |actor: &str, token: &str| SystemCmd::CreateInvite {
        invite_key: crate::proof::invite_key(token),
        invite: invite(true),
        actor: actor.into(),
        consent: Value::Null,
        teams_seen: true,
    };
    let o = m.state.orgs.get_mut("org").unwrap();
    let _prev = o
        .capabilities
        .insert("bob".into(), crate::types::caps::INVITE_MEMBERS);
    assert_eq!(
        m.run_unproven(create("bob", "by-bob")),
        Err(super::ONLY_ADMINS_ALL_APPS.to_owned())
    );
    let _ok = m.run_unproven(create("alice", "all")).unwrap();
    // Bob, a member already, redeems it: a change, so a use is spent.
    let _ok = m.run_unproven(redemption("all", "bob", 1)).unwrap();
    let key = crate::proof::invite_key("all");
    assert_eq!(m.state.invites[&key].uses_left, 1);
    assert!(m.state.orgs["org"].all_apps.contains("bob"));
    // Again: nothing changes, nothing is spent.
    let _ok = m.run_unproven(redemption("all", "bob", 2)).unwrap();
    assert_eq!(m.state.invites[&key].uses_left, 1);
    // Removed from the organization, the grant goes with them.
    let _ok = m
        .run_unproven(SystemCmd::RemoveOrgMember {
            org: "org".into(),
            account: "bob".into(),
            actor: "alice".into(),
            consent: Value::Null,
        })
        .unwrap();
    assert!(m.state.orgs["org"].all_apps.is_empty());
    let back = serde_json::to_value(&m.state.orgs["org"]).unwrap();
    assert!(back.get("allApps").is_none(), "{back}");
}

/// Each app team made from now on gives a row to the members who get
/// every app added later, whoever else it names: one `ScopeApps` makes
/// (closed with an empty list, too), one a move makes in the target, and
/// a team marked as an app team (unless they were removed from it). An
/// app team already there gets nobody. The same entries as an earlier
/// release logged them (no `all_apps_rows`) replay filling no such rows.
#[test]
fn a_new_app_team_gives_its_all_apps_members_a_row() {
    let fresh = || {
        let (mut m, _alice, _bob, _carol) = proven_cluster();
        chat_org(&mut m);
        let s = &mut m.state;
        let _prev = s.bundles.insert(
            "bn".into(),
            serde_json::from_value(json!({
                "hash": "bn", "size": 1, "uploadedBy": "alice",
                "package": { "package": "com.notes", "version": "1.0.0" }
            }))
            .unwrap(),
        );
        let _prev = s.packages.insert(
            "com.notes".into(),
            serde_json::from_value(json!({
                "name": "com.notes", "signerId": "did:key:zN", "owner": "alice",
                "versions": { "1.0.0": {
                    "version": "1.0.0", "mpk": "00", "mpkSize": 1, "source": "upload",
                    "services": { "": "bn" }, "installedBy": "alice", "installedAt": 0
                } }
            }))
            .unwrap(),
        );
        let _prev = s.apps.insert(
            "notes".into(),
            serde_json::from_value(json!({
                "appId": "notes", "name": "notes", "group": 8, "bundle": "bn",
                "owner": "alice", "org": "org", "access": { "kind": "org" },
                "replicas": [1, 2, 3], "createdAt": 0
            }))
            .unwrap(),
        );
        let _new = s
            .orgs
            .get_mut("org")
            .unwrap()
            .all_apps
            .insert("dave".into());
        m
    };
    let closed = |package: &str, flag: bool| SystemCmd::ScopeApps {
        org: "org".into(),
        apps: Some(vec![super::ScopeApp {
            package: package.into(),
            name: None,
            instances: None,
            join: Some(TeamJoin::Invite),
            keep: Some(Vec::new()),
            child_teams: None,
        }]),
        shared: None,
        nonce: "n".into(),
        actor: "alice".into(),
        consent: Value::Null,
        owner_in_team: true,
        accounts_in_app_team: true,
        member_child_teams: true,
        all_apps_rows: flag,
        by_release: true,
    };
    for flag in [true, false] {
        let mut m = fresh();
        let out = m.run_unproven(closed("com.notes", flag)).unwrap();
        let team = out["teams"][0]["teamId"].as_str().unwrap().to_owned();
        let s = &m.state;
        let t = &s.orgs["org"].teams[&team];
        assert_eq!(t.join, TeamJoin::Invite);
        assert_eq!(t.members.contains_key("dave"), flag, "{flag}");
        assert_eq!(s.may_reach(&s.apps["notes"], Some("dave")), flag, "{flag}");
        assert!(!s.may_reach(&s.apps["notes"], Some("carol")));
    }
    // Chat's team was there already: closing it again adds nobody.
    let mut m = fresh();
    let _instance = m.state.apps.insert("chat".into(), {
        let mut a = m.state.apps["notes"].clone();
        a.app_id = "chat".into();
        a.bundle = "bc".into();
        a
    });
    let _ok = m.run_unproven(closed("com.chat", true)).unwrap();
    assert!(!m.state.orgs["org"].teams["chat"]
        .members
        .contains_key("dave"));

    // A team marked as an app team now.
    let mark = |team: &str, flag: bool| SystemCmd::UpdateTeam {
        org: "org".into(),
        team: team.into(),
        name: None,
        join: None,
        parent: None,
        metadata: Some(json!({ "app": { "package": "com.notes" } })),
        actor: "alice".into(),
        all_apps_rows: flag,
        close_drops_below: true,
    };
    for flag in [true, false] {
        let mut m = fresh();
        let _ok = m.run_unproven(mark("t", flag)).unwrap();
        let t = &m.state.orgs["org"].teams["t"];
        assert_eq!(t.members.contains_key("dave"), flag, "{flag}");
    }
    let mut m = fresh();
    let _new = m
        .state
        .orgs
        .get_mut("org")
        .unwrap()
        .teams
        .get_mut("t")
        .unwrap()
        .removed
        .insert("dave".into());
    let _ok = m.run_unproven(mark("t", true)).unwrap();
    assert!(!m.state.orgs["org"].teams["t"].members.contains_key("dave"));

    // A move into an organization makes its app team there.
    for flag in [true, false] {
        let (mut m, _alice, _bob) = moving();
        let target = m.state.orgs.get_mut("new").unwrap();
        let _prev = target.members.insert("bob".into(), OrgRole::Member);
        let _prev = target.members.insert("carol".into(), OrgRole::Member);
        let _new = target.all_apps.insert("carol".into());
        let mut cmd = move_app("oa", "new", "alice", false, Value::Null);
        if let SystemCmd::MoveApp { all_apps_rows, .. } = &mut cmd {
            *all_apps_rows = flag;
        }
        let out = m.run_unproven(cmd).unwrap();
        let team = out["access"]["team"].as_str().unwrap();
        let t = &m.state.orgs["new"].teams[team];
        assert_eq!(t.members.contains_key("carol"), flag, "{flag}");
        assert!(t.members.contains_key("bob"), "the owner, as before");
    }
    // As an earlier release logged them, without the flag: left out.
    let back = serde_json::to_value(closed("com.notes", false)).unwrap();
    assert!(back.get("all_apps_rows").is_none(), "{back}");
}

/// `org` (alice its owner, bob, carol and dave members) with Chat's
/// restricted app team (alice its lead, bob in it) and a release of
/// `com.chat` whose WASM is `bc`.
fn chat_org(m: &mut SystemMachine) {
    let o = m.state.orgs.get_mut("org").unwrap();
    let _prev = o.members.insert("carol".into(), OrgRole::Member);
    let _prev = o.members.insert("dave".into(), OrgRole::Member);
    let _prev = o.teams.insert(
        "chat".into(),
        serde_json::from_value(json!({
            "teamId": "chat", "name": "Chat", "join": "invite",
            "members": { "alice": true, "bob": false },
            "metadata": { "app": { "package": "com.chat" } }
        }))
        .unwrap(),
    );
    let _prev = m.state.bundles.insert(
        "bc".into(),
        serde_json::from_value(json!({
            "hash": "bc", "size": 1, "uploadedBy": "alice",
            "package": { "package": "com.chat", "version": "1.0.0" }
        }))
        .unwrap(),
    );
    let _prev = m.state.packages.insert(
        "com.chat".into(),
        serde_json::from_value(json!({
            "name": "com.chat", "signerId": "did:key:zC", "owner": "alice",
            "versions": { "1.0.0": {
                "version": "1.0.0", "mpk": "00", "mpkSize": 1, "source": "upload",
                "services": { "": "bc" }, "installedBy": "alice", "installedAt": 0
            } }
        }))
        .unwrap(),
    );
}

/// A member who writes a team whose default capabilities hold
/// `CREATE_CHILD_TEAM` (an app team's) creates a restricted team below
/// it and leads it; never an open one, never from a read-only row,
/// without the capability, outside the team, or in an entry an earlier
/// release logged. Removed from the parent, they leave the child too.
#[test]
fn a_member_of_a_team_creates_a_private_team_below_it() {
    use crate::types::{caps, TeamJoin};

    let (mut m, _alice, _bob, _carol) = proven_cluster();
    let o = m.state.orgs.get_mut("org").unwrap();
    let _prev = o.members.insert("carol".into(), OrgRole::Member);
    let t = o.teams.get_mut("t").unwrap();
    let _prev = t.members.insert("bob".into(), false);
    t.default_capabilities = Some(caps::CREATE_CHILD_TEAM);
    let create = |id: &str, who: &str, join: TeamJoin, flag: bool| SystemCmd::CreateTeam {
        org: "org".into(),
        team_id: id.into(),
        name: id.into(),
        parent: Some("t".into()),
        join,
        actor: who.into(),
        member_child_teams: flag,
    };
    let refused = "not allowed to create this team";
    assert_eq!(
        m.run_authorized(create("old", "bob", TeamJoin::Invite, false)),
        Err(refused),
        "as logged before"
    );
    assert_eq!(
        m.run_authorized(create("open", "bob", TeamJoin::Everyone, true)),
        Err(refused)
    );
    assert_eq!(
        m.run_authorized(create("c", "carol", TeamJoin::Invite, true)),
        Err(refused),
        "not in the team"
    );
    let _ok = m
        .run_authorized(create("dm", "bob", TeamJoin::Invite, true))
        .unwrap();
    let o = &m.state.orgs["org"];
    assert_eq!(o.teams["dm"].parent.as_deref(), Some("t"));
    assert!(o.manages_team("bob", "dm") && !o.manages_team("bob", "t"));

    // Read-only, or without the capability: no.
    let t = m
        .state
        .orgs
        .get_mut("org")
        .unwrap()
        .teams
        .get_mut("t")
        .unwrap();
    let _new = t.read_only.insert("bob".into());
    assert_eq!(
        m.run_authorized(create("ro", "bob", TeamJoin::Invite, true)),
        Err(refused)
    );
    let t = m
        .state
        .orgs
        .get_mut("org")
        .unwrap()
        .teams
        .get_mut("t")
        .unwrap();
    let _was = t.read_only.remove("bob");
    t.default_capabilities = Some(caps::INVITE_MEMBERS);
    assert_eq!(
        m.run_authorized(create("nocap", "bob", TeamJoin::Invite, true)),
        Err(refused)
    );

    // Removed from the parent, Bob is out of the team he made below it.
    let _ok = m
        .run_authorized(SystemCmd::RemoveTeamMember {
            org: "org".into(),
            team: "t".into(),
            account: "bob".into(),
            actor: "alice".into(),
            consent: Value::Null,
            subtree: true,
        })
        .unwrap();
    let o = &m.state.orgs["org"];
    assert!(!o.in_team("bob", "dm") && !o.manages_team("bob", "dm"));
}

/// Closing an open team with a plain `UpdateTeam` takes, from whoever
/// loses it (in it by inheritance alone), their rows in the teams below
/// it, as a removal from the team does; whoever keeps it (a direct row)
/// keeps them. An `UpdateTeam` an earlier release logged (no
/// `close_drops_below`) replays keeping every row, as it did.
#[test]
fn closing_a_team_takes_the_rows_below_of_whoever_loses_it() {
    let fresh = || {
        let (mut m, _alice, _bob, _carol) = proven_cluster();
        let o = m.state.orgs.get_mut("org").unwrap();
        let _prev = o.members.insert("carol".into(), OrgRole::Member);
        let _prev = o.teams.insert(
            "chat".into(),
            serde_json::from_value(json!({
                "teamId": "chat", "name": "Chat", "join": "everyone",
                "members": { "alice": true, "bob": false }
            }))
            .unwrap(),
        );
        // Carol, in Chat by inheritance, leads a private channel below
        // it that Bob is in.
        let _prev = o.teams.insert(
            "dm".into(),
            serde_json::from_value(json!({
                "teamId": "dm", "name": "dm", "join": "invite", "parent": "chat",
                "members": { "carol": true, "bob": false }
            }))
            .unwrap(),
        );
        m
    };
    let close = |flag: bool| SystemCmd::UpdateTeam {
        org: "org".into(),
        team: "chat".into(),
        name: None,
        join: Some(TeamJoin::Invite),
        parent: None,
        metadata: None,
        actor: "alice".into(),
        all_apps_rows: true,
        close_drops_below: flag,
    };
    let mut m = fresh();
    assert!(m.state.orgs["org"].in_team("carol", "chat"));
    let _ok = m.run_authorized(close(true)).unwrap();
    let o = &m.state.orgs["org"];
    assert!(!o.in_team("carol", "chat"));
    assert!(!o.in_team("carol", "dm") && !o.manages_team("carol", "dm"));
    assert!(o.in_team("bob", "dm"), "Bob keeps Chat, and the channel");
    assert!(!o.teams["chat"].removed.contains("carol"), "not barred");

    // As logged before: the rows below stay.
    let mut m = fresh();
    let _ok = m.run_authorized(close(false)).unwrap();
    let o = &m.state.orgs["org"];
    assert!(!o.in_team("carol", "chat") && o.in_team("carol", "dm"));
    let logged: SystemCmd = serde_json::from_value(json!({
        "op": "updateTeam", "org": "org", "team": "chat", "join": "invite",
        "actor": "alice"
    }))
    .unwrap();
    assert!(matches!(
        logged,
        SystemCmd::UpdateTeam {
            close_drops_below: false,
            ..
        }
    ));
}

/// One account's rows in several teams in one entry, with one passkey
/// consent over exactly those rows: each team judged as its own
/// `SetTeamMember` (or `RemoveTeamMember`) would be, and the whole batch
/// refused, nothing changed, when any one is. A batch names 1 to
/// `MAX_TEAM_BATCH` teams, each once.
#[test]
fn one_accounts_rows_in_several_teams_are_set_and_taken_at_once() {
    use super::{TeamRow, MAX_TEAM_BATCH};
    use crate::types::TeamRole;

    let (mut m, mut alice, mut bob, _carol) = proven_cluster();
    let o = m.state.orgs.get_mut("org").unwrap();
    let _prev = o.members.insert("carol".into(), OrgRole::Member);
    for (id, parent) in [("chat", None), ("blocks", None), ("dm", Some("chat"))] {
        let _prev = o.teams.insert(
            id.into(),
            serde_json::from_value(json!({
                "teamId": id, "name": id, "join": "invite", "parent": parent,
                "members": { "bob": true }
            }))
            .unwrap(),
        );
    }
    let row = |team: &str, role: Option<TeamRole>| TeamRow {
        team: team.into(),
        role,
        capabilities: None,
    };
    let set = |actor: &str, teams: Vec<TeamRow>| SystemCmd::SetTeamMembers {
        org: "org".into(),
        account: "carol".into(),
        teams,
        actor: actor.into(),
        consent: Value::Null,
    };
    let remove = |actor: &str, teams: &[&str]| SystemCmd::RemoveTeamMembers {
        org: "org".into(),
        account: "carol".into(),
        teams: teams.iter().map(|t| (*t).to_owned()).collect(),
        actor: actor.into(),
        consent: Value::Null,
    };
    let rows = |m: &SystemMachine| -> Vec<String> {
        let o = &m.state.orgs["org"];
        ["t", "chat", "blocks", "dm"]
            .into_iter()
            .filter(|t| o.teams[*t].members.contains_key("carol"))
            .map(str::to_owned)
            .collect()
    };

    // Bob leads Chat and Blocks, not `t`: a batch naming `t` is refused
    // whole, though the other two would be his to set.
    let refused = set(
        "bob",
        vec![row("chat", None), row("blocks", None), row("t", None)],
    );
    assert_eq!(
        m.run_authorized(refused),
        Err("not allowed to manage this team")
    );
    assert!(rows(&m).is_empty(), "nothing set");
    for (teams, why) in [
        (vec![], "name at least one team"),
        (
            vec![row("chat", None), row("chat", None)],
            "a team is named twice",
        ),
        (
            (0..=MAX_TEAM_BATCH)
                .map(|i| row(&format!("t{i}"), None))
                .collect(),
            "too many teams in one step",
        ),
        (vec![row("nope", None)], "no such team"),
    ] {
        assert_eq!(m.run_authorized(set("alice", teams)), Err(why));
    }
    // Not a member of the organization: no row anywhere.
    let mut outsider = set("alice", vec![row("chat", None)]);
    if let SystemCmd::SetTeamMembers { account, .. } = &mut outsider {
        *account = "erin".into();
    }
    assert_eq!(
        m.run_authorized(outsider),
        Err("only organization members join teams")
    );

    // With one passkey consent over exactly the rows: all three at once.
    let teams = vec![
        row("chat", None),
        row("blocks", Some(TeamRole::ReadOnly)),
        row("dm", Some(TeamRole::Lead)),
    ];
    let args = super::team_members_args("carol", &teams);
    let mut fewer = teams.clone();
    let _last = fewer.pop();
    needs_consent(
        &mut m,
        &mut alice,
        ("org:org", "__set_team_members"),
        &args,
        &super::team_members_args("carol", &fewer),
        |consent| SystemCmd::SetTeamMembers {
            org: "org".into(),
            account: "carol".into(),
            teams: teams.clone(),
            actor: "alice".into(),
            consent,
        },
    );
    assert_eq!(rows(&m), ["chat", "blocks", "dm"]);
    let o = &m.state.orgs["org"];
    assert_eq!(o.team_role("carol", "blocks"), Some(TeamRole::ReadOnly));
    assert!(o.manages_team("carol", "dm") && !o.manages_team("carol", "chat"));

    // Taking them: Bob may not take her from `t`, so nothing goes.
    let _ok = m
        .run_authorized(set("alice", vec![row("t", None)]))
        .unwrap();
    assert_eq!(
        m.run_authorized(remove("bob", &["chat", "t"])),
        Err("not allowed to manage this team")
    );
    assert_eq!(rows(&m), ["t", "chat", "blocks", "dm"]);
    // Out of Chat and Blocks with one consent: the channel below Chat
    // goes too, and `t` stays.
    needs_consent(
        &mut m,
        &mut bob,
        ("org:org", "__remove_team_members"),
        &super::remove_team_members_args("carol", &["chat".into(), "blocks".into()]),
        &super::remove_team_members_args("carol", &["chat".into()]),
        |consent| SystemCmd::RemoveTeamMembers {
            org: "org".into(),
            account: "carol".into(),
            teams: vec!["chat".into(), "blocks".into()],
            actor: "bob".into(),
            consent,
        },
    );
    assert_eq!(rows(&m), ["t"]);
    let o = &m.state.orgs["org"];
    assert!(o.teams["chat"].removed.contains("carol"));
    // She leaves `t` herself, in a batch of one.
    let _ok = m.run_authorized(remove("carol", &["t"])).unwrap();
    assert!(rows(&m).is_empty());
}

// ------------------------------------- an org app's owner, while a member

use std::collections::{BTreeMap, BTreeSet};

use super::OwnerRule;
use crate::types::{Access, Team, TeamJoin};

/// `proven_cluster` with carol a member too and an app `oa` of the
/// organization that bob owns.
fn org_app_of_bob() -> (SystemMachine, Soft, Soft) {
    let (mut m, alice, bob, _carol) = proven_cluster();
    let o = m.state.orgs.get_mut("org").unwrap();
    let _prev = o.members.insert("carol".into(), OrgRole::Member);
    let _prev = m.state.apps.insert(
        "oa".into(),
        serde_json::from_value(json!({
            "appId": "oa", "name": "oa", "group": 5, "bundle": "b1", "owner": "bob",
            "org": "org", "access": { "kind": "owner" }, "replicas": [1, 2, 3], "createdAt": 0
        }))
        .unwrap(),
    );
    (m, alice, bob)
}

fn set_access(app: &str, actor: &str) -> SystemCmd {
    SystemCmd::SetAccess {
        app_id: app.into(),
        access: Access::Public,
        actor: actor.into(),
        owner_in_org: true,
        owner_in_team: true,
    }
}

fn set_app_metadata(app: &str, actor: &str) -> SystemCmd {
    SystemCmd::SetAppMetadata {
        app_id: app.into(),
        metadata: json!({ "pinned": true }),
        actor: actor.into(),
        owner_in_org: true,
        owner_in_team: true,
    }
}

fn delete_app(app: &str, actor: &str) -> SystemCmd {
    SystemCmd::DeleteApp {
        app_id: app.into(),
        actor: actor.into(),
        owner_in_org: true,
        owner_in_team: true,
    }
}

fn transfer_app(pk: &mut Soft, app: &str, actor: &str, to: &str) -> SystemCmd {
    SystemCmd::TransferApp {
        app_id: app.into(),
        to: to.into(),
        actor: actor.into(),
        consent: consent(pk, app, "__transfer", &json!({ "to": to })),
        owner_in_org: true,
        owner_in_team: true,
    }
}

// ------------------------------------------ moving an app to another organization

/// `proven_cluster` with a package `p` (bundle `b1`), carol a guest of
/// `org`, the app team `pt` of `p` there (closed) with bob and carol in
/// it, an app `oa` of `p` that bob owns on it, and a second organization
/// `new` that alice owns, holding an open app team of another package
/// (so it is standalone).
fn moving() -> (SystemMachine, Soft, Soft) {
    let (mut m, alice, bob, _carol) = proven_cluster();
    let s = &mut m.state;
    let _prev = s.packages.insert(
        "p".into(),
        serde_json::from_value(json!({
            "name": "p", "signerId": "did:key:zp", "owner": "alice",
            "versions": { "1.0.0": {
                "version": "1.0.0", "mpk": "00", "mpkSize": 1, "source": "upload",
                "services": { "": "b1" }, "metadata": { "name": "Pad" },
                "installedBy": "alice", "installedAt": 0
            } }
        }))
        .unwrap(),
    );
    let _prev = s.bundles.insert(
        "b1".into(),
        serde_json::from_value(json!({
            "hash": "b1", "size": 1, "uploadedBy": "alice",
            "package": { "package": "p", "version": "1.0.0" }
        }))
        .unwrap(),
    );
    let o = s.orgs.get_mut("org").unwrap();
    let _prev = o.members.insert("carol".into(), OrgRole::Guest);
    let _prev = o.teams.insert(
        "pt".into(),
        serde_json::from_value(json!({
            "teamId": "pt", "name": "Pad", "join": "invite",
            "members": { "bob": false, "carol": false },
            "metadata": { "app": { "package": "p", "name": "Pad" } }
        }))
        .unwrap(),
    );
    let _prev = s.orgs.insert(
        "new".into(),
        serde_json::from_value(json!({
            "orgId": "new", "name": "New", "members": { "alice": "owner" },
            "teams": { "qt": {
                "teamId": "qt", "name": "Q", "join": "everyone", "members": {},
                "metadata": { "app": { "package": "q", "name": "Q" } }
            } }
        }))
        .unwrap(),
    );
    let _prev = s.apps.insert(
        "oa".into(),
        serde_json::from_value(json!({
            "appId": "oa", "name": "oa", "group": 5, "bundle": "b1", "owner": "bob",
            "org": "org", "access": { "kind": "team", "team": "pt" },
            "replicas": [1, 2, 3], "createdAt": 0
        }))
        .unwrap(),
    );
    (m, alice, bob)
}

fn move_app(app: &str, to: &str, actor: &str, bring: bool, consent: Value) -> SystemCmd {
    SystemCmd::MoveApp {
        app_id: app.into(),
        to_org: to.into(),
        actor: actor.into(),
        consent,
        bring_members: bring,
        nonce: "n".into(),
        all_apps_rows: true,
        accounts_in_app_team: true,
    }
}

fn granted_run(payer: &str) -> std::collections::BTreeMap<String, super::RunSlot> {
    std::collections::BTreeMap::from([(
        "r".to_owned(),
        super::RunSlot {
            payer: payer.into(),
            fuel: 1,
            seq: 1,
            granted: true,
            refused: false,
            pool: String::new(),
        },
    )])
}

/// A move takes its manager's passkey consent over exactly its target
/// and whether members come along, once.
#[test]
fn moving_an_app_needs_its_managers_consent_over_the_target() {
    let (mut m, mut alice, _bob) = moving();
    needs_consent(
        &mut m,
        &mut alice,
        ("oa", "__move"),
        &super::move_app_args("new", true),
        &super::move_app_args("new", false),
        |c| move_app("oa", "new", "alice", true, c),
    );
    assert_eq!(m.state.apps["oa"].org.as_deref(), Some("new"));
}

/// The app moves with its data's bill, its access and the people who
/// reach it: the target pays its files and runs, a new open app team of
/// its package holds it (alice, who moved it, leads it), bob comes in as
/// a member and carol, a guest, as a guest, each with a row there, and
/// the invites into the team it left are revoked once no app of the
/// source is on it; an organization-wide invite stays.
#[test]
fn a_moved_app_takes_its_payer_access_and_people_to_the_target() {
    let (mut m, mut alice, _bob) = moving();
    m.state.set_app_file_bytes("oa", 100);
    let _prev = m.state.compute.insert("oa".into(), granted_run("org"));
    keep_invite(&mut m, "into-pad", "member", &["pt"], 1);
    keep_invite(&mut m, "org-wide", "member", &[], 1);

    let args = super::move_app_args("new", true);
    let out = m
        .run(move_app(
            "oa",
            "new",
            "alice",
            true,
            consent(&mut alice, "oa", "__move", &args),
        ))
        .unwrap();
    assert_eq!(out["created"], true, "{out}");
    assert_eq!(out["added"], json!(["bob", "carol"]), "{out}");
    assert_eq!(out["revokedInvites"], 1, "{out}");
    let s = &m.state;
    let app = &s.apps["oa"];
    assert_eq!(app.org.as_deref(), Some("new"));
    let crate::types::Access::Team { team } = &app.access else {
        panic!("{:?}", app.access);
    };
    let o = &s.orgs["new"];
    let t = &o.teams[team];
    assert_eq!(t.app_package(), Some("p"));
    assert_eq!(t.name, "Pad");
    assert_eq!(t.join, crate::types::TeamJoin::Everyone);
    assert_eq!(t.members.get("alice"), Some(&true));
    assert_eq!(t.members.get("bob"), Some(&false));
    assert_eq!(t.members.get("carol"), Some(&false));
    assert_eq!(o.role("alice"), Some(OrgRole::Owner));
    assert_eq!(o.role("bob"), Some(OrgRole::Member));
    assert_eq!(o.role("carol"), Some(OrgRole::Guest));
    for who in ["alice", "bob", "carol"] {
        assert!(s.may_reach(app, Some(who)), "{who}");
    }
    assert_eq!((s.file_bytes("org"), s.file_bytes("new")), (0, 100));
    assert_eq!(s.compute["oa"]["r"].payer, "new");
    assert!(!s
        .invites
        .contains_key(&crate::proof::invite_key("into-pad")));
    assert!(s
        .invites
        .contains_key(&crate::proof::invite_key("org-wide")));
    // Its source keeps its own team and people.
    assert!(s.orgs["org"].teams.contains_key("pt"));
    assert_eq!(s.orgs["org"].role("bob"), Some(OrgRole::Member));
}

/// Without `bring_members` nobody joins the target, so an app whose
/// owner is not a member there is refused: an organization's app is owned
/// by one of its members, and only the owner's own consented transfer
/// changes who that is. Once the owner is a member it moves, and those
/// outside the target stop reaching it.
#[test]
fn an_app_moves_without_its_people_only_to_where_its_owner_is() {
    let (mut m, _alice, _bob) = moving();
    let e = m
        .run_unproven(move_app("oa", "new", "alice", false, Value::Null))
        .unwrap_err();
    assert_eq!(e, super::MOVE_OWNER_NOT_IN_TARGET);
    let _prev = m
        .state
        .orgs
        .get_mut("new")
        .unwrap()
        .members
        .insert("bob".into(), OrgRole::Member);
    let out = m
        .run_unproven(move_app("oa", "new", "alice", false, Value::Null))
        .unwrap();
    assert_eq!(out["added"], json!([]), "{out}");
    let s = &m.state;
    assert_eq!(s.orgs["new"].role("carol"), None);
    assert!(!s.may_reach(&s.apps["oa"], Some("carol")));
    assert!(s.may_reach(&s.apps["oa"], Some("bob")));

    // Into a target whose app teams are all closed, without its people:
    // the owner still reaches (and manages) it, with a row of their own.
    let (mut m, _alice, _bob) = moving();
    let target = m.state.orgs.get_mut("new").unwrap();
    target.teams.get_mut("qt").unwrap().join = crate::types::TeamJoin::Invite;
    let _prev = target.members.insert("bob".into(), OrgRole::Member);
    let out = m
        .run_unproven(move_app("oa", "new", "alice", false, Value::Null))
        .unwrap();
    let team = out["access"]["team"].as_str().unwrap().to_owned();
    let s = &m.state;
    assert_eq!(
        s.orgs["new"].teams[&team].join,
        crate::types::TeamJoin::Invite
    );
    assert_eq!(s.orgs["new"].teams[&team].members.get("bob"), Some(&false));
    assert!(s.may_reach(&s.apps["oa"], Some("bob")));
    assert!(s.owner_acts(&s.apps["oa"], "bob"));
}

/// Who moves: the app's manager where it is now, as an admin of the
/// target. A guest, an owner removed from (or who left) the source, an
/// admin of the target demoted to member, and a member of the target
/// who manages nothing are each refused, on the state of the moment, and
/// the app stays where it was.
#[test]
fn only_the_apps_manager_as_the_targets_admin_moves_it() {
    let (mut m, _alice, _bob) = moving();
    let set_role = |m: &mut SystemMachine, org: &str, who: &str, role: Option<OrgRole>| {
        let o = m.state.orgs.get_mut(org).unwrap();
        match role {
            Some(r) => {
                let _prev = o.members.insert(who.into(), r);
            }
            None => {
                let _gone = o.members.remove(who);
            }
        }
    };
    let refused = |m: &mut SystemMachine, actor: &str, why: &str| {
        let e = m
            .run_unproven(move_app("oa", "new", actor, true, Value::Null))
            .unwrap_err();
        assert_eq!(e, why, "{actor}");
        assert_eq!(m.state.apps["oa"].org.as_deref(), Some("org"), "{actor}");
    };
    // Carol, a guest of the source, an admin of the target: no manager.
    set_role(&mut m, "new", "carol", Some(OrgRole::Admin));
    refused(&mut m, "carol", super::ONLY_MANAGERS_MOVE);
    // Bob owns it, but is only a member of the target.
    set_role(&mut m, "new", "bob", Some(OrgRole::Member));
    refused(&mut m, "bob", super::ONLY_ADMINS_MOVE_IN);
    // Made an admin there, then removed from the source (or gone of his
    // own accord: the same row gone): no longer its owner in effect.
    set_role(&mut m, "new", "bob", Some(OrgRole::Admin));
    set_role(&mut m, "org", "bob", None);
    refused(&mut m, "bob", super::ONLY_MANAGERS_MOVE);
    // Back as a member: he moves it.
    set_role(&mut m, "org", "bob", Some(OrgRole::Member));
    let _ok = m
        .run_unproven(move_app("oa", "new", "bob", true, Value::Null))
        .unwrap();
    assert_eq!(m.state.apps["oa"].org.as_deref(), Some("new"));
    // Moving it to where it is is refused.
    assert!(m
        .run_unproven(move_app("oa", "new", "alice", true, Value::Null))
        .is_err());
}

/// The target pays from the move on, so its quotas must hold what the
/// app holds: its files, and its granted runs (count and fuel). A move
/// past either is refused and changes nothing.
#[test]
fn a_move_past_the_targets_quota_is_refused() {
    let (mut m, _alice, _bob) = moving();
    m.state.quotas.max_file_bytes_per_org = 150;
    m.state.set_app_file_bytes("oa", 100);
    let _prev = m.state.apps.insert("na".into(), {
        let mut a = m.state.apps["oa"].clone();
        a.app_id = "na".into();
        a.org = Some("new".into());
        a.access = crate::types::Access::Org;
        a.owner = "alice".into();
        a
    });
    m.state.set_app_file_bytes("na", 60);
    let e = m
        .run_unproven(move_app("oa", "new", "alice", true, Value::Null))
        .unwrap_err();
    assert!(e.starts_with(QUOTA_PREFIX), "{e}");
    m.state.set_app_file_bytes("na", 50);

    m.state.quotas.max_concurrent_runs_per_org = 1;
    let _prev = m.state.compute.insert("oa".into(), granted_run("org"));
    let _prev = m.state.compute.insert("na".into(), granted_run("new"));
    let e = m
        .run_unproven(move_app("oa", "new", "alice", true, Value::Null))
        .unwrap_err();
    assert!(e.starts_with(QUOTA_PREFIX), "{e}");
    let s = &m.state;
    assert_eq!(s.apps["oa"].org.as_deref(), Some("org"));
    assert_eq!((s.file_bytes("org"), s.file_bytes("new")), (100, 50));
    assert_eq!(s.orgs["new"].role("bob"), None, "nobody joined");

    m.state.quotas.max_concurrent_runs_per_org = 2;
    let _ok = m
        .run_unproven(move_app("oa", "new", "alice", true, Value::Null))
        .unwrap();
    assert_eq!(m.state.file_bytes("new"), 150);
}

/// An app on a team other than its package's app team (a team below it,
/// or a plain team) is refused: those rows, leads and capabilities are
/// the source's grants, which the target's admins never saw. So is a
/// move to an organization that approves none of the releases holding
/// the app now.
#[test]
fn a_move_off_a_sub_team_or_onto_other_releases_is_refused() {
    let (mut m, _alice, _bob) = moving();
    let o = m.state.orgs.get_mut("org").unwrap();
    let _prev = o.teams.insert(
        "sub".into(),
        serde_json::from_value(json!({
            "teamId": "sub", "name": "Sub", "parent": "pt", "join": "invite",
            "members": { "bob": true }
        }))
        .unwrap(),
    );
    for team in ["sub", "t"] {
        m.state.apps.get_mut("oa").unwrap().access =
            crate::types::Access::Team { team: team.into() };
        let e = m
            .run_unproven(move_app("oa", "new", "alice", true, Value::Null))
            .unwrap_err();
        assert_eq!(e, super::MOVE_NEEDS_APP_TEAM, "{team}");
    }
    m.state.apps.get_mut("oa").unwrap().access = crate::types::Access::Org;
    m.state
        .orgs
        .get_mut("org")
        .unwrap()
        .approved_releases
        .insert("2.0.0".into());
    m.state
        .orgs
        .get_mut("new")
        .unwrap()
        .approved_releases
        .insert("3.0.0".into());
    let e = m
        .run_unproven(move_app("oa", "new", "alice", true, Value::Null))
        .unwrap_err();
    assert_eq!(e, super::MOVE_RELEASES_DISJOINT);
    m.state
        .orgs
        .get_mut("new")
        .unwrap()
        .approved_releases
        .insert("2.0.0".into());
    let _ok = m
        .run_unproven(move_app("oa", "new", "alice", true, Value::Null))
        .unwrap();
}

/// The target's mode decides the access: a shared target (some app team
/// closed) gets a closed app team, as its others are; a legacy target
/// (no app team) keeps organization access and gets no team, so moving
/// an app in never changes how it holds its other apps; a target that
/// has the package's app team already uses it.
#[test]
fn a_moved_app_follows_the_targets_mode() {
    let (mut m, _alice, _bob) = moving();
    m.state
        .orgs
        .get_mut("new")
        .unwrap()
        .teams
        .get_mut("qt")
        .unwrap()
        .join = crate::types::TeamJoin::Invite;
    let out = m
        .run_unproven(move_app("oa", "new", "alice", true, Value::Null))
        .unwrap();
    let team = out["access"]["team"].as_str().unwrap().to_owned();
    let t = &m.state.orgs["new"].teams[&team];
    assert_eq!(t.join, crate::types::TeamJoin::Invite);
    assert_eq!(
        t.default_capabilities,
        Some(crate::types::caps::CREATE_CHILD_TEAM)
    );
    assert!(m.state.may_reach(&m.state.apps["oa"], Some("bob")));

    // A second instance goes onto the same team.
    let _prev = m.state.apps.insert("ob".into(), {
        let mut a = m.state.apps["oa"].clone();
        a.app_id = "ob".into();
        a.org = Some("org".into());
        a.access = crate::types::Access::Team { team: "pt".into() };
        a
    });
    let out = m
        .run_unproven(move_app("ob", "new", "alice", true, Value::Null))
        .unwrap();
    assert_eq!(out["created"], false, "{out}");
    assert_eq!(out["access"]["team"], team, "{out}");

    // Into a legacy organization: organization access, no team.
    let _prev = m.state.orgs.insert(
        "leg".into(),
        serde_json::from_value(json!({
            "orgId": "leg", "name": "Leg", "members": { "alice": "owner" }, "teams": {}
        }))
        .unwrap(),
    );
    let out = m
        .run_unproven(move_app("ob", "leg", "alice", true, Value::Null))
        .unwrap();
    assert_eq!(out["access"], json!({ "kind": "org" }), "{out}");
    assert!(m.state.orgs["leg"].teams.is_empty());
    assert!(m.state.may_reach(&m.state.apps["ob"], Some("bob")));

    // One app team closed alone (another still open): the team made for
    // the moved app is open, as the other apps' are.
    let (mut m, _alice, _bob) = moving();
    let target = m.state.orgs.get_mut("new").unwrap();
    target.teams.get_mut("qt").unwrap().join = crate::types::TeamJoin::Invite;
    let _prev = target.teams.insert(
        "rt".into(),
        serde_json::from_value(json!({
            "teamId": "rt", "name": "R", "join": "everyone", "members": {},
            "metadata": { "app": { "package": "r", "name": "R" } }
        }))
        .unwrap(),
    );
    let out = m
        .run_unproven(move_app("oa", "new", "alice", true, Value::Null))
        .unwrap();
    let team = out["access"]["team"].as_str().unwrap().to_owned();
    let t = &m.state.orgs["new"].teams[&team];
    assert_eq!(t.join, crate::types::TeamJoin::Everyone);
    assert_eq!(
        t.default_capabilities,
        Some(crate::types::caps::INVITE_MEMBERS | crate::types::caps::CREATE_CHILD_TEAM)
    );
    assert_eq!(
        m.state.orgs["new"].teams["rt"].join,
        crate::types::TeamJoin::Everyone
    );
}

/// An `Accounts` instance (a DM) of a package whose app team in the
/// target is closed is reached there only by those in that team: a move
/// with its people gives each one who reaches it now (bob, a member, and
/// carol, a guest) a plain row on that team, and they reach it after. A
/// move an earlier release logged gives none; an open team needs none.
#[test]
fn a_moved_dm_brings_its_people_into_the_targets_closed_app_team() {
    let dm: AppRecord = serde_json::from_value(json!({
        "appId": "dm", "name": "dm", "group": 6, "bundle": "b1", "owner": "bob",
        "org": "org", "access": { "kind": "accounts", "accounts": ["bob", "carol"] },
        "replicas": [1, 2, 3], "createdAt": 0
    }))
    .unwrap();
    let np: Team = serde_json::from_value(json!({
        "teamId": "np", "name": "Pad", "join": "invite", "members": { "alice": true },
        "metadata": { "app": { "package": "p", "name": "Pad" } }
    }))
    .unwrap();
    let (mut m, _alice, _bob) = moving();
    let s = &mut m.state;
    let _prev = s.apps.insert("dm".into(), dm.clone());
    let _prev = s
        .orgs
        .get_mut("new")
        .unwrap()
        .teams
        .insert("np".into(), np.clone());
    let out = m
        .run_unproven(move_app("dm", "new", "alice", true, Value::Null))
        .unwrap();
    assert_eq!(out["added"], json!(["bob", "carol"]), "{out}");
    let s = &m.state;
    let team = &s.orgs["new"].teams["np"];
    assert_eq!(team.members.get("alice"), Some(&true));
    assert_eq!(team.members.get("bob"), Some(&false));
    assert_eq!(team.members.get("carol"), Some(&false));
    for who in ["bob", "carol"] {
        assert!(s.may_reach(&s.apps["dm"], Some(who)), "{who}");
    }
    assert!(s.owner_acts(&s.apps["dm"], "bob"));

    // The same move as an earlier release logged it (no
    // `accounts_in_app_team`) gives no such rows, as that release did,
    // and the flag is left out of an entry when false.
    let (mut m, _alice, _bob) = moving();
    let s = &mut m.state;
    let _prev = s.apps.insert("dm".into(), dm.clone());
    let _prev = s
        .orgs
        .get_mut("new")
        .unwrap()
        .teams
        .insert("np".into(), np.clone());
    let mut cmd = move_app("dm", "new", "alice", true, Value::Null);
    if let SystemCmd::MoveApp {
        accounts_in_app_team,
        ..
    } = &mut cmd
    {
        *accounts_in_app_team = false;
    }
    let logged = serde_json::to_value(&cmd).unwrap();
    assert!(logged.get("accounts_in_app_team").is_none(), "{logged}");
    let _ok = m
        .run_unproven(serde_json::from_value(logged).unwrap())
        .unwrap();
    let s = &m.state;
    assert_eq!(
        s.orgs["new"].teams["np"].members.keys().collect::<Vec<_>>(),
        ["alice"]
    );
    assert!(!s.may_reach(&s.apps["dm"], Some("bob")));

    // An open app team in the target: its members reach the DM already,
    // and nothing is added.
    let (mut m, _alice, _bob) = moving();
    let s = &mut m.state;
    let _prev = s.apps.insert("dm".into(), dm);
    let open = Team {
        join: TeamJoin::Everyone,
        ..np
    };
    let _prev = s
        .orgs
        .get_mut("new")
        .unwrap()
        .teams
        .insert("np".into(), open);
    let _ok = m
        .run_unproven(move_app("dm", "new", "alice", true, Value::Null))
        .unwrap();
    let s = &m.state;
    assert_eq!(s.orgs["new"].teams["np"].members.len(), 1);
    assert!(s.may_reach(&s.apps["dm"], Some("bob")));
}

/// A personal app moves into an organization its owner administers,
/// keeping its access, and its files go to the organization's bill. It
/// is its owner's alone to move, and only into an organization they
/// administer.
#[test]
fn a_personal_app_moves_into_an_organization_its_owner_administers() {
    let (mut m, _alice, _bob) = moving();
    m.state.set_app_file_bytes("app", 7);
    let before = m.state.apps["app"].access.clone();
    let _prev = m
        .state
        .orgs
        .get_mut("new")
        .unwrap()
        .members
        .insert("bob".into(), OrgRole::Admin);
    let e = m
        .run_unproven(move_app("app", "new", "bob", false, Value::Null))
        .unwrap_err();
    assert_eq!(e, super::ONLY_MANAGERS_MOVE);
    let e = m
        .run_unproven(move_app("app", "org", "alice", false, Value::Null))
        .map(|_| ());
    assert!(e.is_ok(), "alice owns org: {e:?}");
    assert_eq!(m.state.apps["app"].org.as_deref(), Some("org"));
    assert_eq!(m.state.apps["app"].access, before);
    assert_eq!(m.state.file_bytes("org"), 7);
    assert_eq!(m.state.file_bytes("account:alice"), 0);

    // Bob's own app, into an organization where he is only a member.
    let _prev = m.state.apps.insert("bp".into(), {
        let mut a = m.state.apps["app"].clone();
        a.app_id = "bp".into();
        a.org = None;
        a.owner = "bob".into();
        a
    });
    let e = m
        .run_unproven(move_app("bp", "org", "bob", false, Value::Null))
        .unwrap_err();
    assert_eq!(e, super::ONLY_ADMINS_MOVE_IN);
    let _ok = m
        .run_unproven(move_app("bp", "new", "bob", false, Value::Null))
        .unwrap();
}

/// A `MoveApp` entry that names neither `bring_members` nor `nonce`
/// reads as a move without members, and the state it applies to has no
/// new field: an app's record moved reads and writes as any other.
#[test]
fn a_move_entry_without_its_optional_fields_reads_as_a_plain_move() {
    let cmd: SystemCmd = serde_json::from_value(json!({
        "op": "moveApp", "app_id": "oa", "to_org": "new", "actor": "alice"
    }))
    .unwrap();
    let SystemCmd::MoveApp {
        bring_members,
        nonce,
        consent,
        ..
    } = &cmd
    else {
        panic!("{cmd:?}");
    };
    assert!(!bring_members);
    assert!(nonce.is_empty());
    assert!(consent.is_null());
    let (mut m, _alice, _bob) = moving();
    let _prev = m
        .state
        .orgs
        .get_mut("new")
        .unwrap()
        .members
        .insert("bob".into(), OrgRole::Member);
    let _ok = m.run_unproven(cmd).unwrap();
    let record = serde_json::to_value(&m.state.apps["oa"]).unwrap();
    let back: AppRecord = serde_json::from_value(record.clone()).unwrap();
    assert_eq!(serde_json::to_value(&back).unwrap(), record);
}

/// An organization's app is managed by its owner only while they are a
/// member of the organization (`member` or above); once removed, or made
/// a guest, they change its access, metadata or owner, delete it and
/// upgrade it no more.
#[test]
fn an_org_apps_owner_manages_it_only_while_a_member() {
    let (mut m, _alice, mut bob) = org_app_of_bob();
    assert!(m.state.owner_acts(&m.state.apps["oa"], "bob"));
    assert!(m.state.may_upgrade(&m.state.apps["oa"], "bob"));
    let _ok = m.run(set_app_metadata("oa", "bob")).unwrap();

    for demoted in [Some(OrgRole::Guest), None] {
        let o = m.state.orgs.get_mut("org").unwrap();
        match demoted {
            Some(role) => {
                let _prev = o.members.insert("bob".into(), role);
            }
            None => {
                let _gone = o.members.remove("bob");
            }
        }
        let app = &m.state.apps["oa"];
        assert!(!m.state.owner_acts(app, "bob"), "{demoted:?}");
        assert!(!m.state.manages_app(app, "bob"), "{demoted:?}");
        assert!(!m.state.may_upgrade(app, "bob"), "{demoted:?}");
        assert!(m.run(set_access("oa", "bob")).is_err(), "{demoted:?}");
        assert!(m.run(set_app_metadata("oa", "bob")).is_err(), "{demoted:?}");
        assert!(m.run(delete_app("oa", "bob")).is_err(), "{demoted:?}");
        let transfer = transfer_app(&mut bob, "oa", "bob", "carol");
        assert!(m.run(transfer).is_err(), "{demoted:?}");
    }
    let app = &m.state.apps["oa"];
    assert_eq!((app.owner.as_str(), &app.access), ("bob", &Access::Owner));
}

/// An app on a team is reached and managed by its owner only while they
/// are in that team: an owner removed from it, who left it, or who lost
/// it when it closed, reaches it, changes it, deletes it and upgrades it
/// no more, and the organization's admins transfer it. Back in the team,
/// they own it again. An entry an earlier release logged (no
/// `owner_in_team`) replays judging the owner without the team.
#[test]
fn an_apps_owner_reaches_and_manages_an_app_on_a_team_only_while_in_it() {
    let (mut m, mut alice, mut bob) = org_app_of_bob();
    let team = |members: &[&str], join: TeamJoin| Team {
        team_id: "t".into(),
        name: "chat".into(),
        parent: None,
        join,
        members: members.iter().map(|a| ((*a).to_owned(), false)).collect(),
        read_only: BTreeSet::new(),
        capabilities: BTreeMap::new(),
        metadata: json!({ "app": { "package": "com.chat" } }),
        removed: BTreeSet::new(),
        default_capabilities: None,
    };
    let set = |m: &mut SystemMachine, t: Team| {
        let _prev = m
            .state
            .orgs
            .get_mut("org")
            .unwrap()
            .teams
            .insert("t".into(), t);
    };
    set(&mut m, team(&["bob"], TeamJoin::Invite));
    m.state.apps.get_mut("oa").unwrap().access = Access::Team { team: "t".into() };
    let app = m.state.apps["oa"].clone();
    assert!(m.state.may_reach(&app, Some("bob")));
    assert!(m.state.owner_acts(&app, "bob"));
    let _ok = m.run(set_app_metadata("oa", "bob")).unwrap();

    // Kicked or left (no row), closed without them (no row, no longer
    // open), or barred from the open team (`removed`).
    let mut barred = team(&[], TeamJoin::Everyone);
    let _new = barred.removed.insert("bob".into());
    for out in [team(&[], TeamJoin::Invite), barred] {
        set(&mut m, out);
        let app = &m.state.apps["oa"];
        assert!(!m.state.may_reach(app, Some("bob")));
        assert!(!m.state.owner_acts(app, "bob"));
        assert!(!m.state.manages_app(app, "bob"));
        assert!(!m.state.may_upgrade(app, "bob"));
        assert!(m.run(set_access("oa", "bob")).is_err());
        assert!(m.run(set_app_metadata("oa", "bob")).is_err());
        assert!(m.run(delete_app("oa", "bob")).is_err());
        assert!(m.run(transfer_app(&mut bob, "oa", "bob", "carol")).is_err());
    }
    // An entry an earlier release logged replays as it judged it.
    let old = |cmd: SystemCmd| {
        let mut v = serde_json::to_value(cmd).unwrap();
        assert_eq!(v["owner_in_team"], json!(true), "{v}");
        let _flag = v.as_object_mut().unwrap().remove("owner_in_team");
        serde_json::from_value::<SystemCmd>(v).unwrap()
    };
    assert!(m
        .state
        .may_upgrade_as_logged(&m.state.apps["oa"], "bob", OwnerRule::InOrg));
    let _ok = m.run(old(set_app_metadata("oa", "bob"))).unwrap();
    // Open to the organization again: Bob, a member, is in it.
    set(&mut m, team(&[], TeamJoin::Everyone));
    assert!(m.state.may_reach(&m.state.apps["oa"], Some("bob")));
    assert!(m.state.owner_acts(&m.state.apps["oa"], "bob"));
    set(&mut m, team(&["carol"], TeamJoin::Invite));
    let _ok = m
        .run(transfer_app(&mut alice, "oa", "alice", "carol"))
        .unwrap();
    assert!(m.state.owner_acts(&m.state.apps["oa"], "carol"));
}

/// The organization's admins clean up an app whose owner left: they
/// transfer it to a member or delete it. A member who neither owns nor
/// administers it does neither.
#[test]
fn an_org_admin_transfers_or_deletes_an_app_whose_owner_left() {
    let (mut m, mut alice, _bob) = org_app_of_bob();
    let mut carol = Soft::new("localhost", "http://localhost:3000");
    let _gone = m.state.orgs.get_mut("org").unwrap().members.remove("bob");
    let _prev = m.state.apps.insert("ob".into(), {
        let mut b = m.state.apps["oa"].clone();
        b.app_id = "ob".into();
        b
    });
    assert!(m
        .run(transfer_app(&mut carol, "oa", "carol", "carol"))
        .is_err());
    assert!(m.run(delete_app("oa", "carol")).is_err());

    let _ok = m
        .run(transfer_app(&mut alice, "oa", "alice", "carol"))
        .unwrap();
    assert_eq!(m.state.apps["oa"].owner, "carol");
    assert!(m.state.owner_acts(&m.state.apps["oa"], "carol"));
    let _ok = m.run(delete_app("ob", "alice")).unwrap();
    assert!(!m.state.apps.contains_key("ob"));
}

/// A member owner of an organization's app, and the owner of a personal
/// app, manage it as before.
#[test]
fn a_member_owner_and_a_personal_owner_manage_their_apps() {
    let (mut m, mut alice, mut bob) = org_app_of_bob();
    let _ok = m.run(set_access("oa", "bob")).unwrap();
    let _ok = m.run(set_app_metadata("oa", "bob")).unwrap();
    let _ok = m.run(transfer_app(&mut bob, "oa", "bob", "carol")).unwrap();
    assert_eq!(m.state.apps["oa"].owner, "carol");

    // `app` is alice's own, outside any organization.
    assert!(m.state.owner_acts(&m.state.apps["app"], "alice"));
    assert!(!m.state.manages_app(&m.state.apps["app"], "bob"));
    let _ok = m.run(set_access("app", "alice")).unwrap();
    let _ok = m.run(set_app_metadata("app", "alice")).unwrap();
    let _ok = m
        .run(transfer_app(&mut alice, "app", "alice", "bob"))
        .unwrap();
    let _ok = m.run(delete_app("app", "bob")).unwrap();
    assert!(!m.state.apps.contains_key("app"));
}

/// App commands an earlier release logged, without `owner_in_org`,
/// replay as that release judged them: ownership alone, so an owner who
/// had left still changed the app then, and an admin who did not own it
/// did not transfer it. The entries this release logs carry the flag.
#[test]
fn app_commands_logged_before_the_membership_rule_replay_as_they_did() {
    let (mut m, mut alice, mut bob) = org_app_of_bob();
    let _gone = m.state.orgs.get_mut("org").unwrap().members.remove("bob");
    let old = |cmd: SystemCmd| {
        let mut v = serde_json::to_value(cmd).unwrap();
        assert_eq!(v["owner_in_org"], json!(true), "{v}");
        let _flag = v.as_object_mut().unwrap().remove("owner_in_org");
        let _flag = v.as_object_mut().unwrap().remove("owner_in_team");
        serde_json::from_value::<SystemCmd>(v).unwrap()
    };
    assert!(!m.state.may_upgrade(&m.state.apps["oa"], "bob"));
    assert!(m
        .state
        .may_upgrade_as_logged(&m.state.apps["oa"], "bob", OwnerRule::Ownership));
    let refused = m
        .run(old(transfer_app(&mut alice, "oa", "alice", "carol")))
        .unwrap_err();
    assert_eq!(refused, "only the owner may transfer");
    let _ok = m.run(old(set_app_metadata("oa", "bob"))).unwrap();
    let _ok = m.run(old(set_access("oa", "bob"))).unwrap();
    assert_eq!(m.state.apps["oa"].access, Access::Public);
    let _ok = m
        .run(old(transfer_app(&mut bob, "oa", "bob", "carol")))
        .unwrap();
    assert_eq!(m.state.apps["oa"].owner, "carol");
    let _gone = m.state.orgs.get_mut("org").unwrap().members.remove("carol");
    let _ok = m.run(old(delete_app("oa", "carol"))).unwrap();
    assert!(!m.state.apps.contains_key("oa"));
}

// ---------------------------------------------------- account recovery

use super::{
    RefreshGrant, ACCOUNT_RECOVERY_PENDING, NO_ACCOUNT_RECOVERY_PENDING, UNKNOWN_RECOVERY_CODE,
};

/// A canonical recovery code and its public key.
fn recovery_code() -> (String, String) {
    let code = calimero_identity::normalize_recovery_code(&calimero_identity::new_recovery_code())
        .unwrap();
    let key = crate::proof::recovery_key(&code);
    (code, key)
}

/// Set `account`'s recovery code with `pk`'s consent.
fn set_code(m: &mut SystemMachine, pk: &mut Soft, account: &str, key: &str) {
    let c = consent(
        pk,
        &format!("account:{account}"),
        "__set_recovery",
        &json!({ "recoveryKey": key }),
    );
    let _ok = m
        .run(SystemCmd::SetAccountRecoveryKey {
            account: account.into(),
            recovery_key: key.into(),
            consent: c,
        })
        .unwrap();
}

/// A passkey registered on a new device, as the start registers one.
fn new_device() -> (Soft, calimero_identity::Credential) {
    let mut device = Soft::new("localhost", "http://localhost:3000");
    let rp = calimero_identity::RelyingParty {
        id: "localhost".into(),
        origins: vec!["http://localhost:3000".into()],
        require_uv: true,
    };
    let challenge = calimero_identity::random::<32>();
    let cred =
        calimero_identity::verify_registration(&rp, &challenge, &device.register(&challenge))
            .unwrap();
    (device, cred)
}

fn start_with(
    key: &str,
    account: &str,
    credential: &calimero_identity::Credential,
    at: u64,
    signature: String,
) -> SystemCmd {
    SystemCmd::StartAccountRecovery {
        recovery_key: key.into(),
        account: account.into(),
        credential: credential.clone(),
        at,
        signature,
    }
}

/// The finish of `account`'s recovery at `at`, opening `record` with
/// `pk`'s sign-in assertion over it.
fn finish_with(
    pk: &mut Soft,
    account: &str,
    at: u64,
    (hash, record): (&str, &crate::types::RefreshRecord),
) -> SystemCmd {
    SystemCmd::FinishAccountRecovery {
        account: account.into(),
        at,
        refresh: RefreshGrant {
            hash: hash.into(),
            record: record.clone(),
        },
        consent: consent(
            pk,
            "session",
            "__sign_in",
            &super::sign_in_args(hash, record),
        ),
        revoke_api_keys: true,
    }
}

/// A recovery code is set with the account's own passkey, replaced by
/// the next, and removed with the passkey too.
#[test]
fn a_recovery_code_is_set_and_removed_with_the_accounts_passkey() {
    let (mut m, mut alice, mut bob, _carol) = proven_cluster();
    let (_code, key) = recovery_code();
    // Another account's passkey does not set alice's code.
    let c = consent(
        &mut bob,
        "account:alice",
        "__set_recovery",
        &json!({ "recoveryKey": key }),
    );
    assert!(m
        .run(SystemCmd::SetAccountRecoveryKey {
            account: "alice".into(),
            recovery_key: key.clone(),
            consent: c,
        })
        .is_err());
    needs_consent(
        &mut m,
        &mut alice,
        ("account:alice", "__set_recovery"),
        &json!({ "recoveryKey": key }),
        &json!({ "recoveryKey": "00" }),
        |consent| SystemCmd::SetAccountRecoveryKey {
            account: "alice".into(),
            recovery_key: key.clone(),
            consent,
        },
    );
    assert_eq!(
        m.state.recovery_keys.get(&key).map(String::as_str),
        Some("alice")
    );
    // A new code replaces the old: one per account.
    let (_code, second) = recovery_code();
    set_code(&mut m, &mut alice, "alice", &second);
    assert_eq!(m.state.recovery_keys.len(), 1);
    assert!(m.state.recovery_keys.contains_key(&second));
    needs_consent(
        &mut m,
        &mut alice,
        ("account:alice", "__remove_recovery"),
        &json!({}),
        &json!({ "all": true }),
        |consent| SystemCmd::RemoveAccountRecoveryKey {
            account: "alice".into(),
            consent,
        },
    );
    assert!(m.state.recovery_keys.is_empty());
}

/// The start carries the code key's signature over the account and the
/// new passkey: a forged one, one for another account or another
/// passkey, and a replay are refused, and the code is spent. One
/// recovery at a time; only a passkey the account has cancels it.
#[test]
fn an_account_recovery_starts_only_with_the_codes_signature_and_the_account_cancels() {
    let (mut m, mut alice, mut bob, _carol) = proven_cluster();
    let (code, key) = recovery_code();
    set_code(&mut m, &mut alice, "alice", &key);
    let (mut device, cred) = new_device();
    let start = |account: &str, signature: String| start_with(&key, account, &cred, 100, signature);

    // Forged: none, another code's key, a signature over another time.
    assert!(m.run(start("alice", String::new())).is_err(), "none");
    let (guess, _) = recovery_code();
    let forged = crate::proof::sign_recover(&guess, "alice", &cred, 100);
    assert!(m.run(start("alice", forged)).is_err(), "another code");
    let later = crate::proof::sign_recover(&code, "alice", &cred, 101);
    assert!(m.run(start("alice", later)).is_err(), "another time");
    // Wrong account: alice's code does not start bob's recovery, nor
    // does a signature for bob start alice's.
    let for_bob = crate::proof::sign_recover(&code, "bob", &cred, 100);
    assert_eq!(
        m.run(start("bob", for_bob.clone())).unwrap_err(),
        UNKNOWN_RECOVERY_CODE
    );
    assert!(m.run(start("alice", for_bob)).is_err(), "signed for bob");
    // Another passkey than the one signed for.
    let (_other_device, other) = new_device();
    let for_other = crate::proof::sign_recover(&code, "alice", &other, 100);
    assert!(m
        .run(start_with(&key, "alice", &cred, 100, for_other))
        .is_err());

    let real = crate::proof::sign_recover(&code, "alice", &cred, 100);
    let out = m.run(start("alice", real.clone())).unwrap();
    assert_eq!(
        out["readyAt"],
        json!(100 + crate::types::RECOVERY_DELAY_SECS)
    );
    assert!(m.state.recovery_keys.is_empty(), "the code is spent");
    assert_eq!(
        m.state.accounts["alice"].passkeys.len(),
        1,
        "no passkey yet"
    );
    // Replay: the code is spent, and the signature used.
    assert!(m.run(start("alice", real)).is_err(), "replay");
    // A second code does not start a second recovery while one is pending.
    let (code2, key2) = recovery_code();
    set_code(&mut m, &mut alice, "alice", &key2);
    let (_d2, cred2) = new_device();
    let second = crate::proof::sign_recover(&code2, "alice", &cred2, 101);
    assert_eq!(
        m.run(start_with(&key2, "alice", &cred2, 101, second))
            .unwrap_err(),
        ACCOUNT_RECOVERY_PENDING
    );

    // Cancel: not bob's passkey, not the pending passkey itself, not for
    // another passkey; alice's, once.
    let cancel = |consent| SystemCmd::CancelAccountRecovery {
        account: "alice".into(),
        credential_id: cred.id.clone(),
        consent,
    };
    let args = json!({ "credentialId": cred.id });
    let by_bob = consent(&mut bob, "account:alice", "__cancel_recovery", &args);
    assert!(m.run(cancel(by_bob)).is_err(), "another account");
    let by_new = consent(&mut device, "account:alice", "__cancel_recovery", &args);
    assert!(m.run(cancel(by_new)).is_err(), "the pending passkey");
    let wrong = consent(
        &mut alice,
        "account:alice",
        "__cancel_recovery",
        &json!({ "credentialId": cred2.id }),
    );
    assert!(m
        .run(SystemCmd::CancelAccountRecovery {
            account: "alice".into(),
            credential_id: cred2.id.clone(),
            consent: wrong,
        })
        .is_err());
    needs_consent(
        &mut m,
        &mut alice,
        ("account:alice", "__cancel_recovery"),
        &args,
        &json!({ "credentialId": "another" }),
        cancel,
    );
    assert!(m.state.account_recoveries.is_empty());
    // Cancelled, so nothing to finish.
    let (_t, hash, record) = refresh_token("alice", &cred.id, "s2");
    assert_eq!(
        m.run(finish_with(
            &mut device,
            "alice",
            u64::MAX,
            (&hash, &record)
        ))
        .unwrap_err(),
        NO_ACCOUNT_RECOVERY_PENDING
    );
}

/// The finish waits out the delay, judged at its own time; only the
/// passkey the start logged signs it, over the session it opens. It adds
/// that passkey, ends every session the account had and opens the new
/// one, once.
#[test]
fn an_account_recovery_finishes_after_the_delay_with_the_new_passkey() {
    let (mut m, mut alice, mut bob, _carol) = proven_cluster();
    // Alice's lost device still has a session.
    let (_t, old_hash, old) = refresh_token("alice", &alice.credential_id(), "s1");
    let c = consent(
        &mut alice,
        "session",
        "__sign_in",
        &super::sign_in_args(&old_hash, &old),
    );
    let _ok = m
        .run(SystemCmd::CreateRefresh {
            hash: old_hash.clone(),
            record: old,
            consent: c,
        })
        .unwrap();
    let (code, key) = recovery_code();
    set_code(&mut m, &mut alice, "alice", &key);
    let (mut device, cred) = new_device();
    let signature = crate::proof::sign_recover(&code, "alice", &cred, 200);
    let _ok = m
        .run(start_with(&key, "alice", &cred, 200, signature))
        .unwrap();
    let ready = 200 + crate::types::RECOVERY_DELAY_SECS;
    let (_t, hash, record) = refresh_token("alice", &cred.id, "s2");

    // Before the delay: refused, and the assertion is not spent.
    let early = finish_with(&mut device, "alice", ready - 1, (&hash, &record));
    assert_eq!(m.run(early).unwrap_err(), RECOVERY_NOT_READY);
    // Not alice's old passkey, nor bob's: only the one the start logged.
    assert!(m
        .run(finish_with(&mut alice, "alice", ready, (&hash, &record)))
        .is_err());
    assert!(m
        .run(finish_with(&mut bob, "alice", ready, (&hash, &record)))
        .is_err());
    // Not a session naming another passkey or bound to a package.
    let (_t, h2, mut other) = refresh_token("alice", &alice.credential_id(), "s3");
    assert!(m
        .run(finish_with(&mut device, "alice", ready, (&h2, &other)))
        .is_err());
    other.passkey = Some(cred.id.clone());
    other.app = Some("pkg".into());
    assert!(m
        .run(finish_with(&mut device, "alice", ready, (&h2, &other)))
        .is_err());
    // Bob's account has no recovery under way.
    let (_t, h3, for_bob) = refresh_token("bob", &cred.id, "s4");
    assert_eq!(
        m.run(finish_with(&mut device, "bob", ready, (&h3, &for_bob)))
            .unwrap_err(),
        NO_ACCOUNT_RECOVERY_PENDING
    );
    // A sign-in assertion over another session does not finish this one.
    let (_t, h4, r4) = refresh_token("alice", &cred.id, "s5");
    let mut crossed = finish_with(&mut device, "alice", ready, (&h4, &r4));
    if let SystemCmd::FinishAccountRecovery { refresh, .. } = &mut crossed {
        *refresh = RefreshGrant {
            hash: hash.clone(),
            record: record.clone(),
        };
    }
    assert!(m.run(crossed).is_err());

    let finish = finish_with(&mut device, "alice", ready, (&hash, &record));
    let out = m.run(finish.clone()).unwrap();
    assert_eq!(out["revoked"], json!(1));
    let a = &m.state.accounts["alice"];
    assert_eq!(a.passkeys.len(), 2);
    assert!(
        a.passkeys
            .iter()
            .any(|c| c.id == cred.id && c.sign_count > 0),
        "the finish's counter is kept"
    );
    assert_eq!(m.state.credentials[&cred.id], "alice");
    assert!(m.state.account_recoveries.is_empty());
    assert!(
        !m.state.refresh.contains_key(&old_hash),
        "the old session ended"
    );
    assert!(m.state.revoked_sessions.contains("s1"));
    assert_eq!(m.state.refresh[&hash].session, "s2", "the new session");
    // Replay: nothing is pending any more.
    assert_eq!(m.run(finish).unwrap_err(), NO_ACCOUNT_RECOVERY_PENDING);
    // The new passkey signs in as any of the account's.
    let (_t, h5, r5) = refresh_token("alice", &cred.id, "s6");
    let c = consent(
        &mut device,
        "session",
        "__sign_in",
        &super::sign_in_args(&h5, &r5),
    );
    let _ok = m
        .run(SystemCmd::CreateRefresh {
            hash: h5,
            record: r5,
            consent: c,
        })
        .unwrap();
}

/// An API key of `account`, kept under `hash`.
fn api_key(m: &mut SystemMachine, hash: &str, account: &str) {
    let record: crate::types::ApiKeyRecord = serde_json::from_value(json!({
        "keyId": hash, "account": account, "label": "ci", "apps": null,
        "methods": null, "readOnly": false, "allowConfirm": false, "expiresAt": 99,
    }))
    .unwrap();
    let _prev = m.state.api_keys.insert(hash.into(), record);
}

/// Start alice's recovery with a new device's passkey: the finish that
/// device signs once the delay passed.
fn ready_recovery(m: &mut SystemMachine, alice: &mut Soft) -> SystemCmd {
    let (code, key) = recovery_code();
    set_code(m, alice, "alice", &key);
    let (mut device, cred) = new_device();
    let signature = crate::proof::sign_recover(&code, "alice", &cred, 200);
    let _ok = m
        .run(start_with(&key, "alice", &cred, 200, signature))
        .unwrap();
    let ready = 200 + crate::types::RECOVERY_DELAY_SECS;
    let (_t, hash, record) = refresh_token("alice", &cred.id, "s2");
    finish_with(&mut device, "alice", ready, (&hash, &record))
}

/// Whoever stole the lost passkey keeps no machine credential: the
/// finish revokes every API key of the account, and no other account's.
#[test]
fn an_account_recovery_revokes_the_accounts_api_keys() {
    let (mut m, mut alice, _bob, _carol) = proven_cluster();
    api_key(&mut m, "a1", "alice");
    api_key(&mut m, "a2", "alice");
    api_key(&mut m, "b1", "bob");
    let finish = ready_recovery(&mut m, &mut alice);
    let out = m.run(finish).unwrap();
    assert_eq!(out["revokedApiKeys"], json!(2), "{out}");
    assert_eq!(
        m.state.api_keys.keys().collect::<Vec<_>>(),
        ["b1"],
        "alice's keys are gone, bob's stays"
    );
}

/// A finish an earlier release logged (no `revoke_api_keys`) replays as
/// that release applied it: the keys stay, so a replica that replays it
/// after a restart agrees with one that applied it then.
#[test]
fn a_finish_logged_before_key_revocation_replays_as_it_did() {
    let (mut m, mut alice, _bob, _carol) = proven_cluster();
    api_key(&mut m, "a1", "alice");
    let finish = ready_recovery(&mut m, &mut alice);
    let mut logged = serde_json::to_value(&finish).unwrap();
    assert_eq!(
        logged.as_object_mut().unwrap().remove("revoke_api_keys"),
        Some(json!(true))
    );
    let old: SystemCmd = serde_json::from_value(logged).unwrap();
    let out = m.run(old).unwrap();
    assert_eq!(out["revokedApiKeys"], json!(0), "{out}");
    assert!(m.state.api_keys.contains_key("a1"));
    assert!(m.state.account_recoveries.is_empty(), "it still finished");
}

/// State written before account recovery reads with no codes and no
/// recovery under way, and writes back the same bytes; state holding
/// both reads back as written.
#[test]
fn state_from_before_account_recovery_reads_and_writes_as_before() {
    let (mut m, mut alice, _bob, _carol) = proven_cluster();
    let before = serde_json::to_value(&m.state).unwrap();
    assert!(before.get("recoveryKeys").is_none());
    assert!(before.get("accountRecoveries").is_none());
    let restored: SystemState = serde_json::from_value(before.clone()).unwrap();
    assert!(restored.recovery_keys.is_empty());
    assert!(restored.account_recoveries.is_empty());
    assert_eq!(serde_json::to_value(&restored).unwrap(), before);
    assert_eq!(
        serde_json::to_vec(&restored).unwrap(),
        serde_json::to_vec(&m.state).unwrap()
    );

    let (code, key) = recovery_code();
    set_code(&mut m, &mut alice, "alice", &key);
    let (_device, cred) = new_device();
    let signature = crate::proof::sign_recover(&code, "alice", &cred, 5);
    let _ok = m
        .run(start_with(&key, "alice", &cred, 5, signature))
        .unwrap();
    let (_code, key2) = recovery_code();
    set_code(&mut m, &mut alice, "alice", &key2);
    let now = serde_json::to_value(&m.state).unwrap();
    assert_eq!(now["recoveryKeys"][&key2], json!("alice"));
    assert_eq!(
        now["accountRecoveries"]["alice"]["readyAt"],
        json!(5 + crate::types::RECOVERY_DELAY_SECS)
    );
    let restored: SystemState = serde_json::from_value(now.clone()).unwrap();
    assert_eq!(serde_json::to_value(&restored).unwrap(), now);
}

/// Nodes and quotas apply only with the operator's signature over
/// exactly that change, once.
#[test]
fn the_fleet_and_quotas_need_the_operators_signature() {
    let (mut m, ..) = proven_cluster();
    assert_eq!(m.state.nodes.len(), 4, "genesis registers the peers");
    let node = crate::types::NodeRecord {
        id: 9,
        addr: "http://n9/".into(),
        zone: "z".into(),
        role: crate::types::NodeRole::Replica,
        pool: String::new(),
    };
    needs_signature(
        &mut m,
        ("cluster", "__register_node"),
        &json!({ "id": 9, "addr": "http://n9/", "zone": "z" }),
        &json!({ "id": 9, "addr": "http://evil/", "zone": "z" }),
        |proof| SystemCmd::RegisterNode {
            node: node.clone(),
            proof,
        },
    );
    assert!(m.state.nodes.contains_key(&9));
    // A job node's role is part of what the operator signs.
    let job = crate::types::NodeRecord {
        id: 10,
        addr: "http://n10/".into(),
        zone: "z".into(),
        role: crate::types::NodeRole::Job,
        pool: String::new(),
    };
    needs_signature(
        &mut m,
        ("cluster", "__register_node"),
        &json!({ "id": 10, "addr": "http://n10/", "zone": "z", "role": "job" }),
        &json!({ "id": 10, "addr": "http://n10/", "zone": "z" }),
        |proof| SystemCmd::RegisterNode {
            node: job.clone(),
            proof,
        },
    );
    assert_eq!(m.state.nodes[&10].role, crate::types::NodeRole::Job);
    needs_signature(
        &mut m,
        ("cluster", "__remove_node"),
        &json!({ "id": 9 }),
        &json!({ "id": 4 }),
        |proof| SystemCmd::RemoveNode { id: 9, proof },
    );
    assert!(!m.state.nodes.contains_key(&9));
    let mut quotas = m.state.quotas.clone();
    quotas.max_accounts = 7;
    let mut other = quotas.clone();
    other.max_accounts = 7000;
    needs_signature(
        &mut m,
        ("cluster", "__set_quotas"),
        &json!({ "quotas": quotas }),
        &json!({ "quotas": other }),
        |proof| SystemCmd::SetQuotas {
            quotas: quotas.clone(),
            proof,
        },
    );
    assert_eq!(m.state.quotas.max_accounts, 7);
}

/// A move is signed once, over its target: the union step applies with
/// the signature and leaves it unspent, the target spends it, and the
/// union replayed after the move is refused.
#[test]
fn a_replica_move_is_signed_once_over_its_target() {
    let (mut m, ..) = proven_cluster();
    let set = |replicas: &[u64], proof: &serde_json::Value| SystemCmd::SetReplicas {
        app_id: "app".into(),
        replicas: replicas.to_vec(),
        proof: proof.clone(),
    };
    let proof = signed("app", "__set_replicas", &json!({ "replicas": [2, 3, 4] }));
    assert!(m.run(set(&[2, 3, 4], &serde_json::Value::Null)).is_err());
    let other = signed("app", "__set_replicas", &json!({ "replicas": [1, 3, 4] }));
    assert!(m.run(set(&[2, 3, 4], &other)).is_err(), "another target");
    let elsewhere = signed("other", "__set_replicas", &json!({ "replicas": [2, 3, 4] }));
    assert!(m.run(set(&[2, 3, 4], &elsewhere)).is_err(), "another app");
    assert!(
        m.run(set(&[1, 2, 3, 4, 9], &proof)).is_err(),
        "not the union"
    );

    let _ok = m.run(set(&[1, 2, 3, 4], &proof)).unwrap();
    assert_eq!(m.state.apps["app"].replicas, [1, 2, 3, 4]);
    let _ok = m.run(set(&[4, 3, 2], &proof)).unwrap();
    assert_eq!(m.state.apps["app"].replicas, [4, 3, 2]);
    assert_eq!(
        m.run(set(&[1, 2, 3, 4], &proof)).unwrap_err(),
        "that consent was used already",
        "the union step replayed after the move"
    );
    assert!(
        m.run(set(&[2, 3, 4], &proof)).is_err(),
        "the final step replayed"
    );
    assert_eq!(m.state.apps["app"].replicas, [4, 3, 2]);
}

/// A restore is the operator's decision over one backup (`<app> /
/// __restore_app / { backup, sha256 }`), spent once. It moves the app to
/// a new group seeded by its first replica, on the bundle the backup's
/// state runs, and only the seed's `FinishRestore` for that group ends it.
#[test]
fn a_restore_is_signed_over_its_backup_and_ends_in_its_own_group() {
    let (mut m, ..) = proven_cluster();
    // The fixture puts the app in group 2 directly; `CreateApp` would have
    // allocated that group and left `next_group` at it.
    m.state.next_group = m.state.apps["app"].group;
    let _prev = m.state.bundles.insert(
        "b0".into(),
        serde_json::from_value(json!({ "hash": "b0", "size": 1, "uploadedBy": "alice" })).unwrap(),
    );
    let source = |name: &str| crate::types::RestoreSource {
        name: name.into(),
        sha256: "ab".into(),
        index: 7,
    };
    let restore =
        |bundle: &str, replicas: &[u64], proof: &serde_json::Value| SystemCmd::RestoreApp {
            app_id: "app".into(),
            replicas: replicas.to_vec(),
            bundle: bundle.into(),
            compute_bundle: None,
            source: source("7.2.x.snap"),
            proof: proof.clone(),
        };
    let args = json!({ "backup": "7.2.x.snap", "sha256": "ab" });
    let proof = signed("app", "__restore_app", &args);
    assert!(m
        .run(restore("b0", &[2, 1, 3], &serde_json::Value::Null))
        .is_err());
    let other = signed(
        "app",
        "__restore_app",
        &json!({ "backup": "8.2.x.snap", "sha256": "ab" }),
    );
    assert!(
        m.run(restore("b0", &[2, 1, 3], &other)).is_err(),
        "another backup"
    );
    let elsewhere = signed("other", "__restore_app", &args);
    assert!(
        m.run(restore("b0", &[2, 1, 3], &elsewhere)).is_err(),
        "another app"
    );
    assert_eq!(
        m.run(restore("gone", &[2, 1, 3], &proof)).unwrap_err(),
        crate::system::RESTORE_BUNDLE_GONE
    );
    assert!(
        m.run(restore("b0", &[2, 9], &proof)).is_err(),
        "an unknown node"
    );
    assert!(
        m.run(restore("b0", &[2, 2], &proof)).is_err(),
        "a node twice"
    );

    let before = m.state.apps["app"].group;
    let out = m.run(restore("b0", &[2, 1, 3], &proof)).unwrap();
    let app = m.state.apps["app"].clone();
    assert_eq!(out["group"], json!(app.group));
    assert!(app.group > before && app.group == m.state.next_group);
    assert_eq!(
        (app.bundle.as_str(), app.replicas.as_slice()),
        ("b0", &[2, 1, 3][..])
    );
    assert_eq!(
        app.restore,
        Some(crate::types::PendingRestore {
            seed: Some(2),
            source: Some(source("7.2.x.snap"))
        })
    );
    assert_eq!(
        m.run(restore("b0", &[2, 1, 3], &proof)).unwrap_err(),
        "that consent was used already"
    );
    let finish = |group: u64| SystemCmd::FinishRestore {
        app_id: "app".into(),
        group,
    };
    assert!(m.run(finish(before)).is_err(), "the group before");
    let _ok = m.run(finish(app.group)).unwrap();
    assert_eq!(m.state.apps["app"].restore, None);
    assert!(m.run(finish(app.group)).is_err(), "nothing under way");
}

/// A cluster founded again keeps who is who and what is installed, takes
/// the founders as its replica nodes, removes the others (job nodes
/// stay), and leaves every app waiting for its own backup on replicas it
/// still has.
#[test]
fn a_cluster_founded_again_keeps_its_people_and_waits_for_its_apps() {
    let (mut m, ..) = proven_cluster();
    register(&mut m, 9, "job");
    let before = m.state.clone();
    let node = |id: u64| crate::types::NodeRecord {
        id,
        addr: format!("http://new{id}/"),
        zone: String::new(),
        role: crate::types::NodeRole::Replica,
        pool: String::new(),
    };
    let mut s = before.clone();
    super::refound(&mut s, &[node(1), node(5)], 3);
    let ids: Vec<u64> = s.nodes.keys().copied().collect();
    assert_eq!(ids, [1, 5, 9]);
    assert_eq!(s.nodes[&1].addr, "http://new1/");
    assert!(s.removed_nodes.contains(&2) && s.removed_nodes.contains(&4));
    assert!(s.node_epoch[&1] > before.node_epoch.get(&1).copied().unwrap_or(0));
    let app = &s.apps["app"];
    assert_eq!(
        app.replicas,
        [1, 5],
        "kept node 1, filled from the founders"
    );
    assert_eq!(
        app.restore,
        Some(crate::types::PendingRestore::AWAITING_BACKUP)
    );
    assert!(app.restore.as_ref().unwrap().awaits_backup());
    assert_eq!(s.accounts.len(), before.accounts.len());
    assert_eq!(s.operator_key, before.operator_key);
    assert_eq!(s.orgs.len(), before.orgs.len());
    assert_eq!(s.next_group, before.next_group);
}

/// An organization of `who` (account -> role), each holding `caps`
/// organization-wide, with `releases` approved.
fn org(members: &[(&str, &str)], caps: &[(&str, u32)], releases: &[&str]) -> crate::types::Org {
    let members: serde_json::Map<String, serde_json::Value> = members
        .iter()
        .map(|(a, r)| ((*a).to_owned(), json!(r)))
        .collect();
    let caps: serde_json::Map<String, serde_json::Value> = caps
        .iter()
        .map(|(a, c)| ((*a).to_owned(), json!(c)))
        .collect();
    serde_json::from_value(json!({
        "orgId": "org", "name": "Docs", "members": members, "capabilities": caps,
        "approvedReleases": releases, "teams": {}
    }))
    .unwrap()
}

fn approve(org: &str, version: &str, actor: &str) -> SystemCmd {
    SystemCmd::SetReleaseApproval {
        scope: ReleaseScope::Org { org: org.into() },
        version: version.into(),
        approved: true,
        actor: actor.into(),
        consent: serde_json::Value::Null,
    }
}

/// `APPROVE_RELEASES`: the owner and admins hold it (as `MANAGE_APPLICATION`),
/// a member only when granted organization-wide; a team grant, a guest,
/// someone outside, a revoked grant and a removed member do not.
#[test]
fn approving_releases_is_a_capability() {
    use crate::types::caps::APPROVE_RELEASES;
    let mut m = SystemMachine::default();
    let _ok = m.run_unproven(genesis(all(Authority::Owner))).unwrap();
    for who in ["alice", "bob", "carol", "dave", "erin", "frank", "gina"] {
        let _ok = m.run_unproven(account(who)).unwrap();
    }
    let mut o = org(
        &[
            ("alice", "owner"),
            ("bob", "member"),
            ("carol", "member"),
            ("dave", "admin"),
            ("frank", "member"),
            ("gina", "guest"),
        ],
        &[("bob", APPROVE_RELEASES), ("gina", APPROVE_RELEASES)],
        &["2.0.0"],
    );
    let _prev = o.teams.insert(
        "t".into(),
        serde_json::from_value(json!({
            "teamId": "t", "name": "T", "join": "invite",
            "members": { "frank": false }, "capabilities": { "frank": APPROVE_RELEASES }
        }))
        .unwrap(),
    );
    let _prev = m.state.orgs.insert("org".into(), o);

    for (actor, may) in [
        ("alice", true),
        ("dave", true),
        ("bob", true),
        ("carol", false),
        ("frank", false),
        ("gina", false),
        ("erin", false),
    ] {
        assert_eq!(
            m.state.approves_releases("org", actor),
            may,
            "{actor} holds it"
        );
        let r = m.run_unproven(approve("org", &format!("2.1.{}", actor.len()), actor));
        assert_eq!(r.is_ok(), may, "{actor} approving: {r:?}");
    }
    assert_eq!(
        m.state.release_approvers("org"),
        vec!["alice".to_owned(), "bob".into(), "dave".into()]
    );

    // Revoked: refused at apply, whatever the API saw before.
    let _ok = m
        .run_unproven(SystemCmd::SetCapabilities {
            org: "org".into(),
            team: None,
            account: "bob".into(),
            capabilities: 0,
            actor: "alice".into(),
            consent: serde_json::Value::Null,
        })
        .unwrap();
    assert!(m.run_unproven(approve("org", "2.2.0", "bob")).is_err());
    // Removed: gone with the membership.
    let _ok = m
        .run_unproven(SystemCmd::SetCapabilities {
            org: "org".into(),
            team: None,
            account: "bob".into(),
            capabilities: APPROVE_RELEASES,
            actor: "alice".into(),
            consent: serde_json::Value::Null,
        })
        .unwrap();
    let _ok = m
        .run_unproven(SystemCmd::RemoveOrgMember {
            org: "org".into(),
            account: "bob".into(),
            actor: "dave".into(),
            consent: serde_json::Value::Null,
        })
        .unwrap();
    assert!(m.run_unproven(approve("org", "2.2.0", "bob")).is_err());

    // An account's own releases stay its own.
    let own = SystemCmd::SetReleaseApproval {
        scope: ReleaseScope::Account,
        version: "2.4.0".into(),
        approved: true,
        actor: "erin".into(),
        consent: serde_json::Value::Null,
    };
    let _ok = m.run_unproven(own).unwrap();
    assert!(m.state.accounts["erin"].approved_releases.contains("2.4.0"));

    // Where the operator holds release approvals, nobody in an org does.
    let mut m = SystemMachine::default();
    let _ok = m.run_unproven(genesis(all(Authority::Operator))).unwrap();
    let _ok = m.run_unproven(account("alice")).unwrap();
    let _prev = m
        .state
        .orgs
        .insert("org".into(), org(&[("alice", "owner")], &[], &["2.0.0"]));
    assert!(!m.state.approves_releases("org", "alice"));
    assert!(m.run_unproven(approve("org", "2.1.0", "alice")).is_err());
}

/// Past a release's deadline, a scope whose newest approved release is
/// older creates, upgrades and installs nothing; one that approved it is
/// untouched, and approving lifts the stop at once. Judged at the time
/// each command carries, so replicas and replays agree.
#[test]
fn a_release_deadline_stops_new_work_until_the_release_is_approved() {
    use super::{PackageVersion, DEADLINE_PREFIX, OPERATOR};
    const T: u64 = 1_000;
    let mut m = SystemMachine::default();
    let _ok = m.run_unproven(genesis(all(Authority::Owner))).unwrap();
    for who in ["alice", "bob"] {
        let _ok = m.run_unproven(account(who)).unwrap();
        if let Some(a) = m.state.accounts.get_mut(who) {
            let _new = a.approved_releases.insert("2.0.0".into());
        }
    }
    let mut a = org(&[("alice", "owner")], &[], &["2.0.0"]);
    a.org_id = "a".into();
    let mut b = org(&[("bob", "owner")], &[], &["2.0.0", "2.1.0"]);
    b.org_id = "b".into();
    let _prev = m.state.orgs.insert("a".into(), a);
    let _prev = m.state.orgs.insert("b".into(), b);
    for hash in ["b1", "b2"] {
        let _prev = m.state.bundles.insert(
            hash.into(),
            serde_json::from_value(json!({ "hash": hash, "size": 1, "uploadedBy": "alice" }))
                .unwrap(),
        );
    }
    let app = |id: &str, org: &str, owner: &str, at: u64| SystemCmd::CreateApp {
        app: Box::new(
            serde_json::from_value(json!({
                "appId": id, "name": id, "group": 0, "bundle": "b1", "owner": owner,
                "org": org, "access": { "kind": "org" }, "replicas": [1], "createdAt": at
            }))
            .unwrap(),
        ),
    };
    let upgrade = |id: &str, actor: &str, at: u64| SystemCmd::UpgradeApp {
        compute_bundle: None,
        app_id: id.into(),
        bundle: "b2".into(),
        by_operator: false,
        actor: actor.into(),
        at,
        run: None,
        consent: serde_json::Value::Null,
        release: None,
        owner_in_org: true,
        owner_in_team: true,
    };
    let install = |by: &str, version: &str, at: u64| SystemCmd::InstallPackage {
        package: "p".into(),
        signer_id: "did:key:z".into(),
        release: serde_json::from_value::<PackageVersion>(json!({
            "version": version, "mpk": version, "mpkSize": 1, "source": "upload",
            "services": { "": "b1" }, "installedBy": by, "installedAt": at
        }))
        .unwrap(),
    };
    let _ok = m.run_unproven(app("old", "a", "alice", 1)).unwrap();

    // The operator announces 2.1.0's deadline; closed input first.
    for bad in ["latest", "", &"9".repeat(80)] {
        assert!(m
            .run_unproven(SystemCmd::SetReleaseDeadline {
                version: bad.into(),
                after: Some(T),
                proof: serde_json::Value::Null,
            })
            .is_err());
    }
    let _ok = m
        .run_unproven(SystemCmd::SetReleaseDeadline {
            version: "2.1.0".into(),
            after: Some(T),
            proof: serde_json::Value::Null,
        })
        .unwrap();

    // Before it, everything goes on.
    let _ok = m.run_unproven(app("early", "a", "alice", T - 1)).unwrap();
    let _ok = m.run_unproven(upgrade("old", "alice", T - 1)).unwrap();
    let _ok = m.run_unproven(install("alice", "1.0.0", T - 1)).unwrap();
    assert!(!m.state.retired("2.0.0", T - 1));

    // From it, the organization that has not approved 2.1.0 starts nothing.
    let refused = |r: Result<serde_json::Value, String>| {
        let e = r.unwrap_err();
        assert!(e.starts_with(DEADLINE_PREFIX) && e.contains("2.1.0"), "{e}");
    };
    refused(m.run_unproven(app("late", "a", "alice", T)));
    refused(m.run_unproven(upgrade("old", "alice", T)));
    refused(m.run_unproven(install("alice", "1.1.0", T)));
    assert!(m.state.apps.contains_key("old"), "its apps stay");
    assert_eq!(
        m.state.stalled_for(Some("a"), "alice", T).unwrap().release,
        "2.1.0"
    );
    assert!(m.state.retired("2.0.0", T) && !m.state.retired("2.1.0", T));

    // The organization that approved it is unaffected.
    let _ok = m.run_unproven(app("theirs", "b", "bob", T)).unwrap();
    // So is the operator's install, and the approval itself.
    let _ok = m.run_unproven(install(OPERATOR, "1.2.0", T)).unwrap();

    // Approving 2.1.0 lifts it at once.
    let _ok = m.run_unproven(approve("a", "2.1.0", "alice")).unwrap();
    let _ok = m.run_unproven(app("late", "a", "alice", T)).unwrap();
    let _ok = m.run_unproven(upgrade("old", "alice", T)).unwrap();

    // A later deadline names the newest release not approved.
    let _ok = m
        .run_unproven(SystemCmd::SetReleaseDeadline {
            version: "2.2.0".into(),
            after: Some(T + 10),
            proof: serde_json::Value::Null,
        })
        .unwrap();
    assert!(m.state.stalled_for(Some("a"), "alice", T + 9).is_none());
    assert_eq!(
        m.state
            .stalled_for(Some("b"), "bob", T + 10)
            .unwrap()
            .release,
        "2.2.0"
    );
    // Lifted by the operator: gone.
    let _ok = m
        .run_unproven(SystemCmd::SetReleaseDeadline {
            version: "2.2.0".into(),
            after: None,
            proof: serde_json::Value::Null,
        })
        .unwrap();
    assert!(m.state.stalled_for(Some("b"), "bob", T + 10).is_none());
}

/// A package signing key, its `did:key`, and a claim of it for `org`.
fn publisher_key(seed: u8) -> (ed25519_dalek::SigningKey, String) {
    let key = ed25519_dalek::SigningKey::from_bytes(&[seed; 32]);
    let did = calimero_bundle::derive_signer_id_did_key(&key.verifying_key().to_bytes());
    (key, did)
}

/// The binding of `signer` to `org`, signed by `key` for `as_org` at
/// `generation`, proposed by `actor`.
fn bind(
    key: &ed25519_dalek::SigningKey,
    signer: &str,
    orgs: (&str, &str),
    generation: u64,
    actor: &str,
) -> SystemCmd {
    bind_on("test", key, signer, orgs, generation, actor)
}

/// [`bind`], signed for (and naming) the cluster `cluster`.
fn bind_on(
    cluster: &str,
    key: &ed25519_dalek::SigningKey,
    signer: &str,
    (org, as_org): (&str, &str),
    generation: u64,
    actor: &str,
) -> SystemCmd {
    use ed25519_dalek::Signer as _;
    let message = super::publisher_binding_message(cluster, as_org, signer, generation);
    SystemCmd::BindPublisher {
        signer_id: signer.into(),
        org: org.into(),
        cluster: cluster.into(),
        proof: super::PublisherProof {
            public_key: calimero_identity::b64url(&key.verifying_key().to_bytes()),
            signature: calimero_identity::b64url(&key.sign(&message).to_bytes()),
        },
        actor: actor.into(),
        consent: serde_json::Value::Null,
    }
}

/// A cluster with a publisher org `pub` (pam owner, bob member), a
/// customer org `cust` (cora owner), the package `p` signed by `signer`
/// (release 1.0.0 ships `b1`, `upgrades` as given), a raw bundle `raw`,
/// and the customer's instances `inst` (of `p`) and `rawapp`.
fn publishing(signer: &str, upgrades: &str) -> SystemMachine {
    let mut m = SystemMachine::new("test".into());
    let _ok = m.run_unproven(genesis(all(Authority::Owner))).unwrap();
    for who in ["pam", "bob", "cora", "dan"] {
        let _ok = m.run_unproven(account(who)).unwrap();
    }
    let mut o = org(&[("pam", "owner"), ("bob", "member")], &[], &["2.0.0"]);
    o.org_id = "pub".into();
    let _prev = m.state.orgs.insert("pub".into(), o);
    let mut o = org(
        &[("cora", "owner"), ("dan", "member")],
        &[],
        &["2.0.0", "1.9.0"],
    );
    o.org_id = "cust".into();
    let _prev = m.state.orgs.insert("cust".into(), o);
    for (hash, package) in [
        ("b1", json!({ "package": "p", "version": "1.0.0" })),
        ("b2", json!({ "package": "p", "version": "1.1.0" })),
        ("raw", json!(null)),
    ] {
        let _prev = m.state.bundles.insert(
            hash.into(),
            serde_json::from_value(json!({
                "hash": hash, "size": 1, "uploadedBy": "cora", "package": package
            }))
            .unwrap(),
        );
    }
    let _prev = m.state.packages.insert(
        "p".into(),
        serde_json::from_value(json!({
            "name": "p", "signerId": signer, "owner": "pam",
            "versions": {
                "1.0.0": {
                    "version": "1.0.0", "mpk": "00", "mpkSize": 1, "source": "upload",
                    "services": { "": "b1" }, "installedBy": "pam", "installedAt": 0,
                    "upgrades": upgrades
                },
                "1.1.0": {
                    "version": "1.1.0", "mpk": "01", "mpkSize": 1, "source": "upload",
                    "services": { "": "b2" }, "installedBy": "pam", "installedAt": 0,
                    "upgrades": upgrades
                }
            }
        }))
        .unwrap(),
    );
    for (id, bundle) in [("inst", "b1"), ("rawapp", "raw")] {
        let _prev = m.state.apps.insert(
            id.into(),
            serde_json::from_value(json!({
                "appId": id, "name": id, "group": 2, "bundle": bundle, "owner": "cora",
                "org": "cust", "access": { "kind": "org" }, "replicas": [1], "createdAt": 0
            }))
            .unwrap(),
        );
    }
    m
}

/// A package signing key bound to an organization makes it the
/// publisher: its approved releases then govern every instance of the
/// key's packages, its customers' included (a raw bundle stays its
/// owner's). The binding is the key's own signature over this cluster,
/// the organization and the key's generation, checked at apply, proposed
/// by an `APPROVE_RELEASES` holder there; binding again takes a new
/// signature, and an old one is never replayed.
#[test]
fn a_publisher_approves_the_releases_of_every_instance_of_its_packages() {
    use crate::types::caps::APPROVE_RELEASES;
    let (key, signer) = publisher_key(7);
    let (other, _) = publisher_key(8);
    let mut m = publishing(&signer, "owner");
    let inst = |m: &SystemMachine| m.state.apps["inst"].clone();
    let rawapp = |m: &SystemMachine| m.state.apps["rawapp"].clone();
    assert_eq!(
        m.state.approved_releases(&inst(&m)),
        Some(&m.state.orgs["cust"].approved_releases),
        "unbound: the owner's set"
    );

    // Forged or misplaced bindings are refused at apply.
    for (cmd, why) in [
        (
            bind(&other, &signer, ("pub", "pub"), 0, "pam"),
            "another key",
        ),
        (
            bind(&key, &signer, ("pub", "cust"), 0, "pam"),
            "signed for another org",
        ),
        (
            bind(&key, &signer, ("pub", "pub"), 1, "pam"),
            "a generation ahead",
        ),
        (
            bind(&key, &signer, ("pub", "pub"), 0, "bob"),
            "a member without the capability",
        ),
        (
            bind(&key, &signer, ("pub", "pub"), 0, "cora"),
            "an outsider",
        ),
    ] {
        assert!(m.run_unproven(cmd).is_err(), "{why}");
    }
    let mut cluster = bind(&key, &signer, ("pub", "pub"), 0, "pam");
    if let SystemCmd::BindPublisher { cluster: c, .. } = &mut cluster {
        *c = "another".into();
    }
    assert!(
        m.run_unproven(cluster).is_err(),
        "signed for another cluster"
    );
    assert!(m.state.publishers.is_empty());

    // A member granted APPROVE_RELEASES binds it.
    if let Some(o) = m.state.orgs.get_mut("pub") {
        let _prev = o.capabilities.insert("bob".into(), APPROVE_RELEASES);
    }
    let _ok = m
        .run_unproven(bind(&key, &signer, ("pub", "pub"), 0, "bob"))
        .unwrap();
    assert_eq!(m.state.publisher_of("b1"), Some(&"pub".to_owned()));
    assert_eq!(m.state.binding_generation(&signer), 1);
    assert_eq!(
        m.state.approved_releases(&inst(&m)),
        Some(&m.state.orgs["pub"].approved_releases)
    );
    assert_eq!(
        m.state.approved_releases(&rawapp(&m)),
        Some(&m.state.orgs["cust"].approved_releases),
        "a raw bundle stays its owner's"
    );

    // The customer approving a release changes nothing for the instance;
    // the publisher approving it does.
    let _ok = m.run_unproven(approve("cust", "2.1.0", "cora")).unwrap();
    assert!(!m.state.release_may_hold(&inst(&m), Some("2.1.0")));
    assert!(m.state.release_may_hold(&rawapp(&m), Some("2.1.0")));
    let _ok = m.run_unproven(approve("pub", "2.1.0", "pam")).unwrap();
    assert!(m.state.release_may_hold(&inst(&m), Some("2.1.0")));

    // Deadlines judge the instance on its publisher's set.
    let _ok = m
        .run_unproven(SystemCmd::SetReleaseDeadline {
            version: "2.2.0".into(),
            after: Some(10),
            proof: serde_json::Value::Null,
        })
        .unwrap();
    let _ok = m.run_unproven(approve("cust", "2.2.0", "cora")).unwrap();
    let create = |id: &str, bundle: &str| SystemCmd::CreateApp {
        app: Box::new(
            serde_json::from_value(json!({
                "appId": id, "name": id, "group": 0, "bundle": bundle, "owner": "cora",
                "org": "cust", "access": { "kind": "org" }, "replicas": [1], "createdAt": 10
            }))
            .unwrap(),
        ),
    };
    let e = m.run_unproven(create("second", "b1")).unwrap_err();
    assert!(e.starts_with(super::DEADLINE_PREFIX), "{e}");
    let _ok = m.run_unproven(create("raw2", "raw")).unwrap();
    let _ok = m.run_unproven(approve("pub", "2.2.0", "pam")).unwrap();
    let _ok = m.run_unproven(create("second", "b1")).unwrap();

    // Rebinding: the old signature is spent; the key signs the next
    // generation for the organization it moves to, and the one it
    // leaves cannot keep it.
    assert!(m
        .run_unproven(bind(&key, &signer, ("pub", "pub"), 0, "pam"))
        .is_err());
    assert!(m
        .run_unproven(bind(&key, &signer, ("cust", "cust"), 0, "cora"))
        .is_err());
    let _ok = m
        .run_unproven(bind(&key, &signer, ("cust", "cust"), 1, "cora"))
        .unwrap();
    assert_eq!(m.state.publisher_of("b1"), Some(&"cust".to_owned()));
    assert_eq!(m.state.binding_generation(&signer), 2);
}

/// A binding is checked against this cluster's own name, never the one
/// the command names: a key's genuine signature for another cluster
/// binds nothing here.
#[test]
fn a_binding_signed_for_another_cluster_binds_nothing_here() {
    let (key, signer) = publisher_key(7);
    let mut m = publishing(&signer, "owner");
    let elsewhere = bind_on("another", &key, &signer, ("pub", "pub"), 0, "pam");
    assert!(m.run_unproven(elsewhere).is_err());
    assert!(m.state.publishers.is_empty());
    assert_eq!(m.state.binding_generation(&signer), 0);
    let _ok = m
        .run_unproven(bind(&key, &signer, ("pub", "pub"), 0, "pam"))
        .unwrap();
    assert_eq!(
        m.state.publisher_of_signer(&signer),
        Some(&"pub".to_owned())
    );
}

/// Deleting an organization drops the keys bound to it: an organization
/// created later under the same id is not their publisher, and each key
/// keeps its generation, so the signature that bound it stays spent.
#[test]
fn a_deleted_organization_takes_its_publisher_bindings_with_it() {
    let (key, signer) = publisher_key(7);
    let mut m = publishing(&signer, "owner");
    let _ok = m
        .run_unproven(bind(&key, &signer, ("pub", "pub"), 0, "pam"))
        .unwrap();
    let _ok = m
        .run_unproven(SystemCmd::DeleteOrg {
            org: "pub".into(),
            actor: "pam".into(),
            consent: Value::Null,
        })
        .unwrap();
    let _ok = m
        .run_unproven(SystemCmd::CreateOrg {
            org_id: "pub".into(),
            name: "pub".into(),
            actor: "dan".into(),
            release: None,
            recovery_key: None,
            at: 0,
            name_fits: true,
        })
        .unwrap();
    assert_eq!(m.state.publisher_of_signer(&signer), None);
    assert_eq!(m.state.publisher_of("b1"), None);
    assert_eq!(m.state.binding_generation(&signer), 1);
    assert!(m
        .run_unproven(bind(&key, &signer, ("pub", "pub"), 0, "dan"))
        .is_err());
    let _ok = m
        .run_unproven(bind(&key, &signer, ("pub", "pub"), 1, "dan"))
        .unwrap();
    assert_eq!(
        m.state.publisher_of_signer(&signer),
        Some(&"pub".to_owned())
    );
    assert_eq!(m.state.binding_generation(&signer), 2);
}

/// An upgrade key's upgrade is judged where it applies, on fresh state:
/// the key's own signature, spent once; refused once its account may no
/// longer upgrade the app (a member who lost `MANAGE_APPLICATION`, or
/// left), once the key is revoked or expired, and for a bundle of another
/// package or of its package reinstalled under another signer.
#[test]
fn an_upgrade_keys_upgrade_holds_only_while_its_maker_may_upgrade() {
    use crate::types::caps::MANAGE_APPLICATION;
    let (_key, signer) = publisher_key(9);
    let mut m = publishing(&signer, "owner");
    if let Some(o) = m.state.orgs.get_mut("cust") {
        let _prev = o.capabilities.insert("dan".into(), MANAGE_APPLICATION);
    }
    let secret = "cal_ci-secret";
    let record = |expires_at: u64| super::ApiKeyRecord {
        key_id: "k1".into(),
        account: "dan".into(),
        label: "ci".into(),
        apps: None,
        methods: None,
        read_only: false,
        allow_confirm: false,
        expires_at,
        upgrades: Some(crate::types::UpgradeScope {
            app: "inst".into(),
            package: "p".into(),
            signer: signer.clone(),
            proof_key: crate::proof::api_key_public(secret),
        }),
    };
    let r = record(100);
    assert_eq!(
        m.state.upgrade_key_fits(&r, r.upgrades.as_ref().unwrap()),
        Ok(())
    );
    assert!(
        m.state
            .upgrade_key_fits(
                &record(100),
                &crate::types::UpgradeScope {
                    app: "rawapp".into(),
                    ..r.upgrades.clone().unwrap()
                }
            )
            .is_err(),
        "an app that runs no release of the package"
    );
    let _prev = m.state.api_keys.insert("h1".into(), r);
    let upgrade = |bundle: &str, at: u64, secret: &str| {
        let statement = crate::proof::Statement::new(
            "inst",
            "__upgrade",
            &json!({ "bundle": bundle, "migrate": null, "manifest": null }),
        );
        SystemCmd::UpgradeApp {
            app_id: "inst".into(),
            bundle: bundle.into(),
            compute_bundle: None,
            by_operator: false,
            actor: "dan".into(),
            at,
            run: None,
            consent: json!(crate::proof::sign_key_upgrade(secret, "k1", statement)),
            release: None,
            owner_in_org: true,
            owner_in_team: true,
        }
    };
    let e = m.run(upgrade("b2", 10, "cal_other")).unwrap_err();
    assert!(e.contains("not signed with the API key"), "{e}");
    let e = m.run(upgrade("raw", 10, secret)).unwrap_err();
    assert!(e.contains("only to signed releases"), "{e}");
    let e = m.run(upgrade("b2", 100, secret)).unwrap_err();
    assert!(e.contains("expired"), "{e}");
    let once = upgrade("b2", 10, secret);
    let _ok = m.run(once.clone()).unwrap();
    assert_eq!(m.state.apps["inst"].bundle, "b2");
    assert_eq!(
        m.state.apps["inst"]
            .last_upgrade
            .as_ref()
            .and_then(|u| u.api_key.as_deref()),
        Some("k1")
    );
    let e = m.run(once).unwrap_err();
    assert!(e.contains("used already"), "{e}");

    // Its maker loses the right to upgrade: so does the key.
    if let Some(o) = m.state.orgs.get_mut("cust") {
        let _prev = o.capabilities.insert("dan".into(), 0);
    }
    assert!(!m.state.may_upgrade(&m.state.apps["inst"], "dan"));
    assert!(m.run(upgrade("b1", 10, secret)).is_err(), "descoped");
    if let Some(o) = m.state.orgs.get_mut("cust") {
        let _prev = o.capabilities.insert("dan".into(), MANAGE_APPLICATION);
    }
    let _ok = m.run(upgrade("b1", 11, secret)).unwrap();

    // The package under another signer, then the key revoked.
    let (_other, other_signer) = publisher_key(10);
    if let Some(p) = m.state.packages.get_mut("p") {
        p.signer_id = other_signer;
    }
    let e = m.run(upgrade("b2", 12, secret)).unwrap_err();
    assert!(e.contains("signer it names"), "{e}");
    if let Some(p) = m.state.packages.get_mut("p") {
        p.signer_id = signer.clone();
    }
    let _ok = m
        .run_unproven(SystemCmd::RevokeApiKey {
            key_id: "k1".into(),
            actor: "dan".into(),
            consent: serde_json::Value::Null,
        })
        .unwrap();
    let e = m.run(upgrade("b2", 13, secret)).unwrap_err();
    assert!(e.contains("revoked"), "{e}");
}

/// A package's manifest says who upgrades its instances. `publisher`:
/// the publisher's `MANAGE_APPLICATION` holders, for every instance, and
/// not the customer; until its key is bound, nobody. The
/// declaration only loosens toward owners, at install and at upgrade.
#[test]
fn a_package_says_whether_its_publisher_or_each_owner_upgrades_it() {
    use super::PackageVersion;
    use crate::types::caps::MANAGE_APPLICATION;
    let (key, signer) = publisher_key(9);
    let mut m = publishing(&signer, "publisher");
    if let Some(o) = m.state.orgs.get_mut("pub") {
        let _prev = o.capabilities.insert("bob".into(), MANAGE_APPLICATION);
    }
    if let Some(o) = m.state.orgs.get_mut("cust") {
        let _prev = o.capabilities.insert("dan".into(), MANAGE_APPLICATION);
    }
    let inst = |m: &SystemMachine| m.state.apps["inst"].clone();
    let may = |m: &SystemMachine, who: &str| m.state.may_upgrade(&inst(m), who);

    // Unbound: nobody upgrades a `publisher` package's instances yet.
    for who in ["pam", "bob", "cora", "dan"] {
        assert!(!may(&m, who), "{who}");
    }

    let _ok = m
        .run_unproven(bind(&key, &signer, ("pub", "pub"), 0, "pam"))
        .unwrap();
    for (who, expected) in [
        ("pam", true),
        ("bob", true),
        ("cora", false),
        ("dan", false),
    ] {
        assert_eq!(may(&m, who), expected, "{who}");
    }
    let upgrade = |actor: &str, bundle: &str| SystemCmd::UpgradeApp {
        compute_bundle: None,
        app_id: "inst".into(),
        bundle: bundle.into(),
        by_operator: false,
        actor: actor.into(),
        at: 0,
        run: None,
        consent: serde_json::Value::Null,
        release: None,
        owner_in_org: true,
        owner_in_team: true,
    };
    assert!(
        m.run_unproven(upgrade("cora", "b2")).is_err(),
        "the customer may not"
    );
    let _ok = m.run_unproven(upgrade("bob", "b2")).unwrap();

    // Releases: publisher first, then owner, never back.
    let install = |version: &str, upgrades: &str| SystemCmd::InstallPackage {
        package: "p".into(),
        signer_id: signer.clone(),
        release: serde_json::from_value::<PackageVersion>(json!({
            "version": version, "mpk": version, "mpkSize": 1, "source": "upload",
            "services": { "": format!("h{version}") }, "installedBy": "pam",
            "installedAt": 0, "upgrades": upgrades
        }))
        .unwrap(),
    };
    let _ok = m.run_unproven(install("1.2.0", "owner")).unwrap();
    assert!(
        m.run_unproven(install("1.3.0", "publisher")).is_err(),
        "back to the publisher"
    );
    assert!(
        m.run_unproven(install("1.1.5", "owner")).is_ok(),
        "owner between them"
    );
    assert!(
        m.run_unproven(install("0.9.0", "owner")).is_err(),
        "owner below a publisher release"
    );
    let _ok = m.run_unproven(install("0.8.0", "publisher")).unwrap();

    // The instance moves to the owner release, by its publisher; then it
    // is its owner's, and never goes back.
    let _prev = m.state.bundles.insert(
        "h1.2.0".into(),
        serde_json::from_value(json!({
            "hash": "h1.2.0", "size": 1, "uploadedBy": "pam",
            "package": { "package": "p", "version": "1.2.0" }
        }))
        .unwrap(),
    );
    let _ok = m.run_unproven(upgrade("pam", "h1.2.0")).unwrap();
    assert!(may(&m, "cora") && !may(&m, "pam"));
    assert!(
        m.run_unproven(upgrade("cora", "b1")).is_err(),
        "never back to the publisher"
    );
    let _ok = m.run_unproven(upgrade("cora", "h1.2.0")).unwrap();
}

// ------------------------------------------------------------ deployers

/// A release of package `p` (`version`, its WASM `b-<version>`)
/// installed by `by` at `at`.
fn install_at(by: &str, version: &str, at: u64) -> SystemCmd {
    SystemCmd::InstallPackage {
        package: "p".into(),
        signer_id: "did:key:z".into(),
        release: serde_json::from_value(json!({
            "version": version, "mpk": version, "mpkSize": 1, "source": "upload",
            "services": { "": format!("b-{version}") }, "installedBy": by, "installedAt": at
        }))
        .unwrap(),
    }
}

fn raw_bundle(actor: &str, hash: &str, at: u64) -> SystemCmd {
    SystemCmd::PutBundle {
        hash: hash.into(),
        size: 1,
        actor: actor.into(),
        at,
    }
}

fn grant(deployer: super::Deployer, until: Option<u64>, actor: &str) -> SystemCmd {
    SystemCmd::GrantDeployer {
        deployer,
        until,
        at: 1,
        actor: actor.into(),
        proof: None,
        consent: None,
    }
}

fn account_deployer(id: &str) -> super::Deployer {
    super::Deployer::Account(id.into())
}

/// A cluster deployed with `deployments`, alice, bob and carol, and
/// organization `org` where bob is a plain member and carol holds
/// `MANAGE_APPLICATION` organization-wide.
fn deploying(deployments: super::Deployments) -> SystemMachine {
    let mut m = SystemMachine::default();
    let _ok = m
        .run_unproven(genesis(Governance {
            deployments,
            ..Governance::default()
        }))
        .unwrap();
    for who in ["alice", "bob", "carol"] {
        let _ok = m.run_unproven(account(who)).unwrap();
    }
    let o = org(
        &[("alice", "owner"), ("bob", "member"), ("carol", "member")],
        &[("carol", crate::types::caps::MANAGE_APPLICATION)],
        &[],
    );
    let _prev = m.state.orgs.insert("org".into(), o);
    m
}

/// Where deployments are granted, an account installs a package or
/// uploads a raw bundle only with a live deployer grant, judged at the
/// install's own time; an organization's grant reaches only its
/// `MANAGE_APPLICATION` holders; the operator always installs. Revoking
/// stops new installs and keeps what is installed.
#[test]
fn on_a_granted_cluster_only_deployers_install() {
    use super::{Deployer, Deployments, NOT_A_DEPLOYER, OPERATOR};
    const T: u64 = 1_000;
    let mut m = deploying(Deployments::Granted);
    assert_eq!(
        m.run_unproven(install_at("alice", "1.0.0", 10))
            .unwrap_err(),
        NOT_A_DEPLOYER
    );
    assert_eq!(
        m.run_unproven(raw_bundle("alice", "raw", 10)).unwrap_err(),
        NOT_A_DEPLOYER
    );
    let _ok = m.run_unproven(install_at(OPERATOR, "0.9.0", 10)).unwrap();

    // An account's own grant, until T: judged at the install's time.
    let _ok = m
        .run_unproven(grant(account_deployer("alice"), Some(T), OPERATOR))
        .unwrap();
    let _ok = m.run_unproven(install_at("alice", "1.0.0", T - 1)).unwrap();
    let _ok = m.run_unproven(raw_bundle("alice", "raw", T - 1)).unwrap();
    assert_eq!(
        m.run_unproven(install_at("alice", "1.1.0", T)).unwrap_err(),
        NOT_A_DEPLOYER,
        "past `until`"
    );
    assert!(
        m.run_unproven(install_at("bob", "1.0.0", 10)).is_err(),
        "the same release again is still an install"
    );

    // An organization's grant: its MANAGE_APPLICATION holders only.
    let _ok = m
        .run_unproven(grant(Deployer::Org("org".into()), None, OPERATOR))
        .unwrap();
    let _ok = m.run_unproven(install_at("carol", "2.0.0", T)).unwrap();
    let _ok = m.run_unproven(install_at("alice", "2.1.0", T)).unwrap();
    assert_eq!(
        m.run_unproven(install_at("bob", "2.2.0", T)).unwrap_err(),
        NOT_A_DEPLOYER,
        "a plain member"
    );

    // Revoked: nothing new, and what is installed stays.
    let _ok = m
        .run_unproven(SystemCmd::RevokeDeployer {
            deployer: Deployer::Org("org".into()),
            at: T,
            actor: OPERATOR.into(),
            proof: None,
            consent: None,
        })
        .unwrap();
    assert!(m.run_unproven(install_at("carol", "2.3.0", T)).is_err());
    assert!(m.state.packages["p"].versions.contains_key("2.0.0"));

    // Grants name an account or organization that exists, end after
    // they are made, and only the operator makes them here.
    for (why, cmd) in [
        ("unknown", grant(account_deployer("nobody"), None, OPERATOR)),
        (
            "already over",
            grant(account_deployer("bob"), Some(1), OPERATOR),
        ),
        (
            "not the operator",
            grant(account_deployer("bob"), None, "alice"),
        ),
    ] {
        assert!(m.run_unproven(cmd).is_err(), "{why}");
    }
}

/// Where deployments are open, installs go on exactly as before.
#[test]
fn on_an_open_cluster_anyone_installs() {
    let mut m = deploying(super::Deployments::Open);
    let _ok = m.run_unproven(install_at("bob", "1.0.0", 10)).unwrap();
    let _ok = m.run_unproven(raw_bundle("bob", "raw", 10)).unwrap();
    assert!(m.state.deployers.is_empty());
    let json = serde_json::to_value(&m.state).unwrap();
    assert!(json.get("deployers").is_none() && json.get("clusterOwner").is_none());
}

/// The operator moves deployments from open to granted once; never back,
/// and never twice.
#[test]
fn deployments_are_granted_one_way() {
    use super::Deployments;
    let mut m = deploying(Deployments::Open);
    let _ok = m
        .run_unproven(SystemCmd::SetDeploymentsGranted {
            proof: serde_json::Value::Null,
        })
        .unwrap();
    assert_eq!(m.state.governance().deployments, Deployments::Granted);
    assert!(m
        .run_unproven(SystemCmd::SetDeploymentsGranted {
            proof: serde_json::Value::Null,
        })
        .is_err());
    assert_eq!(
        m.run_unproven(install_at("bob", "1.0.0", 10)).unwrap_err(),
        super::NOT_A_DEPLOYER
    );
}

/// The operator's key in tests, and its signature over a statement.
fn operator_signed(subject: &str, method: &str, args: &serde_json::Value) -> serde_json::Value {
    use ed25519_dalek::Signer;
    let statement = crate::proof::Statement::new(subject, method, args);
    let key = ed25519_dalek::SigningKey::from_bytes(&[9; 32]);
    let signature = key.sign(&statement.operator_message("c"));
    serde_json::to_value(crate::proof::OperatorProof {
        statement,
        signature: hex::encode(signature.to_bytes()),
    })
    .unwrap()
}

/// Where the cluster's owner grants deployers: the operator grants them
/// until it names the owner, once; from then on the owner's passkey
/// grants them and the operator's signature does not. Every grant carries
/// its proof over exactly that grant, used once.
#[test]
fn deployer_grants_carry_the_operators_signature_then_the_owners_passkey() {
    use super::{Deployers, Deployments, OPERATOR};
    let key = ed25519_dalek::SigningKey::from_bytes(&[9; 32]);
    let mut m = SystemMachine::default();
    let _ok = m
        .run(SystemCmd::Genesis {
            session_proofs: true,
            governance: Governance {
                deployments: Deployments::Granted,
                deployers: Deployers::Owner,
                ..Governance::default()
            },
            release: Some("1.0.0".into()),
            operator_key: Some(crate::proof::OperatorKey {
                key: hex::encode(key.verifying_key().to_bytes()),
                cluster: "c".into(),
            }),
            nodes: Vec::new(),
        })
        .unwrap();
    let mut alice =
        calimero_identity::testing::SoftAuthenticator::new("localhost", "http://localhost:3000");
    let mut bob =
        calimero_identity::testing::SoftAuthenticator::new("localhost", "http://localhost:3000");
    let _ok = m.run(account_with("alice", &mut alice)).unwrap();
    let _ok = m.run(account_with("bob", &mut bob)).unwrap();
    let args = |who: &str| json!({ "deployer": { "account": who }, "until": null });
    let by_operator = |who: &str, proof: serde_json::Value| SystemCmd::GrantDeployer {
        deployer: account_deployer(who),
        until: None,
        at: 1,
        actor: OPERATOR.into(),
        proof: Some(proof),
        consent: None,
    };

    // Before the owner is named, the operator grants, with its key.
    let other = {
        use ed25519_dalek::Signer;
        let statement = crate::proof::Statement::new("cluster", "__grant_deployer", &args("bob"));
        let signature =
            ed25519_dalek::SigningKey::from_bytes(&[8; 32]).sign(&statement.operator_message("c"));
        serde_json::to_value(crate::proof::OperatorProof {
            statement,
            signature: hex::encode(signature.to_bytes()),
        })
        .unwrap()
    };
    for (why, cmd) in [
        ("another key", by_operator("bob", other)),
        (
            "over another deployer",
            by_operator(
                "bob",
                operator_signed("cluster", "__grant_deployer", &args("alice")),
            ),
        ),
        (
            "a revocation's signature",
            by_operator(
                "bob",
                operator_signed("cluster", "__revoke_deployer", &args("bob")),
            ),
        ),
        (
            "a passkey instead",
            SystemCmd::GrantDeployer {
                deployer: account_deployer("bob"),
                until: None,
                at: 1,
                actor: "alice".into(),
                proof: None,
                consent: Some(consent(
                    &mut alice,
                    "cluster",
                    "__grant_deployer",
                    &args("bob"),
                )),
            },
        ),
    ] {
        assert!(m.run(cmd).is_err(), "{why}");
    }
    assert!(m.state.deployers.is_empty());
    let signed = operator_signed("cluster", "__grant_deployer", &args("bob"));
    let _ok = m.run(by_operator("bob", signed.clone())).unwrap();
    assert!(m.state.may_deploy("bob", 5));
    let _ok = m
        .run(SystemCmd::RevokeDeployer {
            deployer: account_deployer("bob"),
            at: 2,
            actor: OPERATOR.into(),
            proof: Some(operator_signed(
                "cluster",
                "__revoke_deployer",
                &json!({ "deployer": { "account": "bob" } }),
            )),
            consent: None,
        })
        .unwrap();
    assert_eq!(
        m.run(by_operator("bob", signed)).unwrap_err(),
        "that consent was used already",
        "a grant replayed after its revocation"
    );
    assert!(!m.state.may_deploy("bob", 5));

    // The operator names alice the owner, once.
    let owner = |who: &str| SystemCmd::SetClusterOwner {
        owner: account_deployer(who),
        proof: operator_signed(
            "cluster",
            "__cluster_owner",
            &json!({ "owner": { "account": who } }),
        ),
    };
    assert!(m
        .run(SystemCmd::SetClusterOwner {
            owner: account_deployer("alice"),
            proof: serde_json::Value::Null,
        })
        .is_err());
    let _ok = m.run(owner("alice")).unwrap();
    assert!(m.run(owner("bob")).is_err(), "a second owner");
    assert_eq!(m.state.cluster_owner, Some(account_deployer("alice")));

    // Now the operator's signature grants nothing; alice's passkey does,
    // and bob's does not.
    let fresh_operator = operator_signed(
        "cluster",
        "__grant_deployer",
        &json!({ "deployer": { "account": "bob" }, "until": 9 }),
    );
    assert!(m
        .run(SystemCmd::GrantDeployer {
            deployer: account_deployer("bob"),
            until: Some(9),
            at: 1,
            actor: OPERATOR.into(),
            proof: Some(fresh_operator),
            consent: None,
        })
        .is_err());
    let by_owner = |actor: &str, pk: &mut calimero_identity::testing::SoftAuthenticator| {
        SystemCmd::GrantDeployer {
            deployer: account_deployer("bob"),
            until: None,
            at: 1,
            actor: actor.into(),
            proof: None,
            consent: Some(consent(pk, "cluster", "__grant_deployer", &args("bob"))),
        }
    };
    assert!(m.run(by_owner("bob", &mut bob)).is_err(), "not the owner");
    assert!(!m.state.may_deploy("bob", 5));
    let _ok = m.run(by_owner("alice", &mut alice)).unwrap();
    assert!(m.state.may_deploy("bob", 5));
    assert_eq!(m.state.deployers["account:bob"].granted_by, "alice");
}

/// A deployer's JSON names its kind, and its key names it back.
#[test]
fn a_deployer_reads_from_its_key() {
    use super::Deployer;
    let d = Deployer::Org("o1".into());
    assert_eq!(serde_json::to_value(&d).unwrap(), json!({ "org": "o1" }));
    assert_eq!(Deployer::from_key(&d.key()), Some(d));
    assert_eq!(Deployer::from_key("team:x"), None);
}

// ------------------------------------------------------------ deployments

fn create_deployment(id: &str, owner: super::Deployer, actor: &str) -> SystemCmd {
    SystemCmd::CreateDeployment {
        id: id.into(),
        name: id.into(),
        owner,
        actor: actor.into(),
        at: 10,
        consent: serde_json::Value::Null,
    }
}

fn offer_app(deployment: &str, app: &str, version: &str, actor: &str) -> SystemCmd {
    SystemCmd::OfferApp {
        deployment: deployment.into(),
        app: app.into(),
        package: "p".into(),
        version: version.into(),
        actor: actor.into(),
        at: 10,
        consent: serde_json::Value::Null,
    }
}

fn withdraw_app(deployment: &str, app: &str, actor: &str) -> SystemCmd {
    SystemCmd::WithdrawApp {
        deployment: deployment.into(),
        app: app.into(),
        actor: actor.into(),
        consent: serde_json::Value::Null,
    }
}

fn delete_deployment(deployment: &str, actor: &str) -> SystemCmd {
    SystemCmd::DeleteDeployment {
        deployment: deployment.into(),
        actor: actor.into(),
        consent: serde_json::Value::Null,
    }
}

/// An instance `id` of `owner`'s running `bundle`, from `deployment`.
fn instance(id: &str, owner: &str, bundle: &str, deployment: Option<&str>) -> SystemCmd {
    SystemCmd::CreateApp {
        app: Box::new(
            serde_json::from_value(json!({
                "appId": id, "name": id, "group": 0, "bundle": bundle, "owner": owner,
                "replicas": [1], "createdAt": 10, "deployment": deployment,
            }))
            .unwrap(),
        ),
    }
}

/// An account's deployment is its own; an organization's is its
/// org-wide `MANAGE_APPLICATION` holders'. Offers name installed
/// releases the manager sees; an instance from a deployment runs exactly
/// an offered release; a withdraw waits for the deployment's own
/// instances; an offered release is not uninstalled; a deployment is
/// deleted once it offers nothing and no instance came from it.
#[test]
fn deployments_are_their_managers_and_keep_what_they_offer() {
    use super::{Deployer, Deployments, MAX_DEPLOYMENTS_PER_OWNER, QUOTA_PREFIX};
    let mut m = deploying(Deployments::Open);
    let alice = || Deployer::Account("alice".into());
    let org = || Deployer::Org("org".into());
    // alice, alone, for herself; carol (MANAGE_APPLICATION) for the org.
    assert!(m
        .run_unproven(create_deployment("a", alice(), "bob"))
        .is_err());
    let _ok = m
        .run_unproven(create_deployment("a", alice(), "alice"))
        .unwrap();
    assert!(m
        .run_unproven(create_deployment("o", org(), "bob"))
        .is_err());
    let _ok = m
        .run_unproven(create_deployment("o", org(), "carol"))
        .unwrap();
    assert!(
        m.run_unproven(create_deployment("o", org(), "alice"))
            .is_err(),
        "exists"
    );
    let bad_name = SystemCmd::CreateDeployment {
        id: "x".into(),
        name: " padded ".into(),
        owner: alice(),
        actor: "alice".into(),
        at: 10,
        consent: serde_json::Value::Null,
    };
    assert!(m.run_unproven(bad_name).is_err());
    assert!(m
        .run_unproven(create_deployment("-x", alice(), "alice"))
        .is_err());

    // Only installed releases, seen by the manager.
    assert!(m
        .run_unproven(offer_app("a", "notes", "1.0.0", "alice"))
        .is_err());
    let _ok = m.run_unproven(install_at("alice", "1.0.0", 5)).unwrap();
    let _ok = m.run_unproven(install_at("alice", "2.0.0", 5)).unwrap();
    let _ok = m
        .run_unproven(offer_app("a", "notes", "1.0.0", "alice"))
        .unwrap();
    assert!(m
        .run_unproven(offer_app("a", "notes", "1.0.0", "bob"))
        .is_err());
    assert!(m
        .run_unproven(offer_app("a", "../x", "1.0.0", "alice"))
        .is_err());
    // bob is a plain member; carol shares the org with alice, so sees it.
    assert!(m
        .run_unproven(offer_app("o", "notes", "1.0.0", "bob"))
        .is_err());
    let _ok = m
        .run_unproven(offer_app("o", "notes", "1.0.0", "carol"))
        .unwrap();
    let _ok = m
        .run_unproven(offer_app("o", "notes", "1.0.0", "alice"))
        .unwrap();
    // An entry keeps its package.
    let mut other = offer_app("a", "notes", "1.0.0", "alice");
    if let SystemCmd::OfferApp { package, .. } = &mut other {
        *package = "q".into();
    }
    assert!(m.run_unproven(other).is_err());

    // Instances: only the offered release, of a deployment that exists.
    let _ok = m
        .run_unproven(instance("i1", "bob", "b-1.0.0", Some("a")))
        .unwrap();
    assert!(m
        .run_unproven(instance("i2", "bob", "b-2.0.0", Some("a")))
        .is_err());
    assert!(m
        .run_unproven(instance("i3", "bob", "b-1.0.0", Some("zz")))
        .is_err());
    let _ok = m
        .run_unproven(instance("i4", "bob", "b-2.0.0", None))
        .unwrap();

    // Withdraw: refused while an instance from this deployment runs the
    // package; the other deployment's entry is its own.
    assert!(m.run_unproven(withdraw_app("a", "notes", "alice")).is_err());
    let _ok = m.run_unproven(withdraw_app("o", "notes", "carol")).unwrap();
    // Offered by `a`: not uninstalled, even by its first installer; a
    // release nobody offers keeps today's rules.
    let uninstall = |v: &str| SystemCmd::UninstallPackage {
        package: "p".into(),
        version: Some(v.into()),
        actor: "alice".into(),
    };
    assert!(m.run_unproven(uninstall("1.0.0")).is_err());
    let _gone = m.state.apps.remove("i1");
    assert!(
        m.run_unproven(delete_deployment("a", "alice")).is_err(),
        "it offers notes"
    );
    let _ok = m.run_unproven(withdraw_app("a", "notes", "alice")).unwrap();
    let _ok = m.run_unproven(uninstall("1.0.0")).unwrap();
    assert!(m.run_unproven(delete_deployment("a", "bob")).is_err());
    let _ok = m.run_unproven(delete_deployment("a", "alice")).unwrap();
    assert!(!m.state.deployments.contains_key("a"));

    // An organization owning a deployment is not deleted under it.
    let delete_org = SystemCmd::DeleteOrg {
        org: "org".into(),
        actor: "alice".into(),
        consent: serde_json::Value::Null,
    };
    assert!(m.run_unproven(delete_org).is_err());

    // A bounded number per owner.
    for i in 1..MAX_DEPLOYMENTS_PER_OWNER {
        let _ok = m
            .run_unproven(create_deployment(&format!("o{i}"), org(), "carol"))
            .unwrap();
    }
    let e = m
        .run_unproven(create_deployment("one-more", org(), "carol"))
        .unwrap_err();
    assert!(e.starts_with(QUOTA_PREFIX), "{e}");
}

/// Where deployments are granted, creating one and offering need a live
/// grant at the command's time; withdrawing does not.
#[test]
fn on_a_granted_cluster_deploying_needs_a_grant() {
    use super::{Deployer, Deployments, NOT_A_DEPLOYER, OPERATOR};
    let mut m = deploying(Deployments::Granted);
    let alice = || Deployer::Account("alice".into());
    assert_eq!(
        m.run_unproven(create_deployment("a", alice(), "alice"))
            .unwrap_err(),
        NOT_A_DEPLOYER
    );
    let _ok = m.run_unproven(grant(alice(), None, OPERATOR)).unwrap();
    let _ok = m.run_unproven(install_at("alice", "1.0.0", 5)).unwrap();
    let _ok = m
        .run_unproven(create_deployment("a", alice(), "alice"))
        .unwrap();
    let _ok = m
        .run_unproven(offer_app("a", "notes", "1.0.0", "alice"))
        .unwrap();
    // A customer without a grant creates from the offer.
    let _ok = m
        .run_unproven(instance("i1", "bob", "b-1.0.0", Some("a")))
        .unwrap();
    let _ok = m
        .run_unproven(SystemCmd::RevokeDeployer {
            deployer: alice(),
            at: 20,
            actor: OPERATOR.into(),
            proof: None,
            consent: None,
        })
        .unwrap();
    assert_eq!(
        m.run_unproven(offer_app("a", "more", "1.0.0", "alice"))
            .unwrap_err(),
        NOT_A_DEPLOYER
    );
    let _gone = m.state.apps.remove("i1");
    let _ok = m.run_unproven(withdraw_app("a", "notes", "alice")).unwrap();
}

/// Each change to a deployment carries its actor's passkey consent over
/// exactly that change, once; a deployment's id is its creating
/// consent's, so no node picks another.
#[test]
fn deployment_changes_carry_their_managers_consent() {
    use super::{deployment_id, Deployer};
    let (mut m, mut alice, mut bob) = two_accounts();
    let owner = json!({ "account": "alice" });
    let args = json!({ "name": "labs", "owner": owner });
    let create =
        |id: String, actor: &str, consent: serde_json::Value| SystemCmd::CreateDeployment {
            id,
            name: "labs".into(),
            owner: Deployer::Account("alice".into()),
            actor: actor.into(),
            at: 10,
            consent,
        };
    assert!(m
        .run(create("d".into(), "alice", serde_json::Value::Null))
        .is_err());
    let c = consent(&mut alice, "cluster", "__create_deployment", &args);
    assert!(
        m.run(create("picked".into(), "alice", c.clone())).is_err(),
        "an id the consent does not name"
    );
    let id = deployment_id(c["nonce"].as_str().unwrap());
    let theirs = consent(&mut bob, "cluster", "__create_deployment", &args);
    let bobs_id = deployment_id(theirs["nonce"].as_str().unwrap());
    assert!(
        m.run(create(bobs_id, "alice", theirs)).is_err(),
        "bob's passkey"
    );
    let _ok = m.run(create(id.clone(), "alice", c.clone())).unwrap();
    assert!(m.run(create(id.clone(), "alice", c)).is_err(), "used once");

    let _ok = m.run_unproven(install_at("alice", "1.0.0", 5)).unwrap();
    let subject = format!("deployment:{id}");
    let offer = |version: &str, consent: serde_json::Value| SystemCmd::OfferApp {
        deployment: id.clone(),
        app: "notes".into(),
        package: "p".into(),
        version: version.into(),
        actor: "alice".into(),
        at: 10,
        consent,
    };
    let for_1 = json!({ "app": "notes", "package": "p", "version": "1.0.0" });
    let c = consent(&mut alice, &subject, "__offer_app", &for_1);
    assert!(m.run(offer("2.0.0", c.clone())).is_err(), "another version");
    let _ok = m.run(offer("1.0.0", c)).unwrap();
    let withdraw = |consent: serde_json::Value| SystemCmd::WithdrawApp {
        deployment: id.clone(),
        app: "notes".into(),
        actor: "alice".into(),
        consent,
    };
    let c = consent(
        &mut alice,
        "deployment:other",
        "__withdraw_app",
        &json!({ "app": "notes" }),
    );
    assert!(m.run(withdraw(c)).is_err(), "another deployment");
    let c = consent(
        &mut alice,
        &subject,
        "__withdraw_app",
        &json!({ "app": "notes" }),
    );
    let _ok = m.run(withdraw(c)).unwrap();
    let c = consent(&mut alice, &subject, "__delete_deployment", &json!({}));
    let _ok = m
        .run(SystemCmd::DeleteDeployment {
            deployment: id.clone(),
            actor: "alice".into(),
            consent: c,
        })
        .unwrap();
    assert!(m.state.deployments.is_empty());
}

/// A system state and an app record written before deployments (log
/// entries and snapshots are JSON) read with none, and write back as
/// they were.
#[test]
fn state_from_before_deployments_reads_with_none_and_writes_as_before() {
    // `AppRecord` as the previous release wrote it, every field present.
    let written: crate::types::AppRecord = serde_json::from_value(json!({
        "appId": "a", "name": "n", "group": 2, "bundle": "b", "owner": "alice",
        "replicas": [1], "createdAt": 0,
    }))
    .unwrap();
    let app = serde_json::to_value(&written).unwrap();
    assert!(app.get("deployment").is_none(), "{app}");
    let record: crate::types::AppRecord = serde_json::from_value(app.clone()).unwrap();
    assert_eq!(record.deployment, None);
    assert_eq!(serde_json::to_value(&record).unwrap(), app);

    let mut m = SystemMachine::default();
    let _ok = m.run_authorized(genesis(all(Authority::Owner))).unwrap();
    let _ok = m.run_authorized(account("alice")).unwrap();
    let _prev = m.state.apps.insert("a".into(), record);
    let before = serde_json::to_value(&m.state).unwrap();
    assert!(before.get("deployments").is_none(), "{before}");
    assert!(before["apps"]["a"].get("deployment").is_none());
    let restored: SystemState = serde_json::from_value(before.clone()).unwrap();
    assert!(restored.deployments.is_empty());
    assert_eq!(serde_json::to_value(&restored).unwrap(), before);
}

/// An app of `org` (or a personal one) in a machine whose file quota is `quota`.
fn machine_with_apps(quota: u64, apps: &[(&str, Option<&str>)]) -> SystemMachine {
    let mut m = SystemMachine::default();
    m.state.quotas.max_file_bytes_per_org = quota;
    for (id, org) in apps {
        let app = serde_json::from_value(json!({
            "appId": id, "name": id, "group": 2, "bundle": "b", "owner": "alice",
            "org": org, "replicas": [1], "createdAt": 0
        }))
        .unwrap();
        let _prev = m.state.apps.insert((*id).into(), app);
        if let Some(org) = org {
            let _prev = m.state.orgs.insert(
                (*org).into(),
                serde_json::from_value(json!({
                    "orgId": org, "name": org, "members": { "alice": "owner" }, "teams": {}
                }))
                .unwrap(),
            );
        }
    }
    m
}

fn charge(app: &str, delta: i64) -> SystemCmd {
    SystemCmd::ChargeFileBytes {
        app_id: app.into(),
        delta,
    }
}

#[test]
fn file_bytes_are_granted_up_to_the_quota_across_the_payers_apps() {
    let mut m = machine_with_apps(100, &[("a", Some("org")), ("b", Some("org")), ("c", None)]);
    let _ok = m.run_unproven(charge("a", 60)).unwrap();
    let _ok = m.run_unproven(charge("b", 40)).unwrap();
    assert_eq!(m.state.file_bytes("org"), 100);

    let refused = m.run_unproven(charge("a", 1)).unwrap_err();
    assert!(refused.starts_with(QUOTA_PREFIX), "{refused}");
    assert_eq!(m.state.file_bytes("org"), 100, "a refusal changes nothing");

    // Another payer has its own quota.
    let _ok = m.run_unproven(charge("c", 100)).unwrap();
    assert_eq!(m.state.file_bytes("account:alice"), 100);
    assert!(m.run_unproven(charge("nobody", 1)).is_err());
}

#[test]
fn a_refund_frees_room_and_stops_at_zero() {
    let mut m = machine_with_apps(100, &[("a", Some("org")), ("b", Some("org"))]);
    let _ok = m.run_unproven(charge("a", 100)).unwrap();
    assert!(m.run_unproven(charge("b", 1)).is_err());
    let _ok = m.run_unproven(charge("a", -30)).unwrap();
    let _ok = m.run_unproven(charge("b", 30)).unwrap();
    assert_eq!(m.state.file_bytes("org"), 100);

    let _ok = m.run_unproven(charge("a", -1_000)).unwrap();
    assert!(!m.state.app_file_bytes.contains_key("a"));
    assert_eq!(m.state.app_file_bytes["b"], 30, "another app's bytes stay");
}

#[test]
fn deleting_an_app_or_an_org_frees_its_files() {
    let mut m = machine_with_apps(100, &[("a", Some("org")), ("b", Some("org")), ("c", None)]);
    for app in ["a", "b", "c"] {
        let _ok = m.run_unproven(charge(app, 10)).unwrap();
    }
    let _ok = m
        .run_unproven(SystemCmd::DeleteApp {
            app_id: "a".into(),
            actor: "alice".into(),
            owner_in_org: true,
            owner_in_team: true,
        })
        .unwrap();
    assert_eq!(m.state.file_bytes("org"), 10);
    assert!(!m.state.app_file_bytes.contains_key("a"));

    let _prev = m.state.orgs.insert(
        "org".into(),
        serde_json::from_value(json!({
            "orgId": "org", "name": "O", "members": { "alice": "owner" }, "teams": {}
        }))
        .unwrap(),
    );
    let _ok = m
        .run_unproven(SystemCmd::DeleteOrg {
            org: "org".into(),
            actor: "alice".into(),
            consent: serde_json::Value::Null,
        })
        .unwrap();
    assert_eq!(m.state.file_bytes("org"), 0);
    assert_eq!(m.state.app_file_bytes.len(), 1, "only the personal app's");
}

#[test]
fn the_file_quota_defaults_to_fifty_gigabytes_and_reads_from_old_state() {
    assert_eq!(Quotas::default().max_file_bytes_per_org, 50_000_000_000);
    let old: Quotas = serde_json::from_value(json!({ "maxAccounts": 7 })).unwrap();
    assert_eq!(old.max_file_bytes_per_org, 50_000_000_000);
    assert_eq!(
        serde_json::to_value(&old).unwrap()["maxFileBytesPerOrg"],
        json!(50_000_000_000u64)
    );
    let old_state: SystemState = serde_json::from_value(json!({
        "nodes": {}, "apps": {}, "nextGroup": 1, "bundles": {}, "accounts": {},
        "credentials": {}, "apiKeys": {}, "refresh": {}, "revokedSessions": [],
        "sessionKeys": [], "orgs": {}, "invites": {}
    }))
    .unwrap();
    assert!(old_state.app_file_bytes.is_empty());
}

/// The per-payer totals equal the sum over the apps' bytes.
fn assert_totals_are_sums(s: &SystemState) {
    let mut sums = std::collections::BTreeMap::new();
    for (id, bytes) in &s.app_file_bytes {
        let payer = crate::usage::payer(&s.apps[id]);
        *sums.entry(payer).or_insert(0u64) += bytes;
    }
    assert_eq!(s.payer_file_bytes, sums);
}

#[test]
fn the_payer_totals_always_equal_the_sum_over_the_apps() {
    let mut m = machine_with_apps(
        1_000,
        &[("a", Some("org")), ("b", Some("org")), ("c", None)],
    );
    let _ok = m.run_unproven(genesis(all(Authority::Owner))).unwrap();
    let _ok = m.run_unproven(account("bob")).unwrap();
    let steps = [
        charge("a", 100),
        charge("b", 50),
        charge("c", 70),
        charge("a", -30),
    ];
    for step in steps {
        let _ok = m.run_unproven(step).unwrap();
        assert_totals_are_sums(&m.state);
    }
    let _ok = m.run_unproven(charge("b", -500)).unwrap();
    assert_totals_are_sums(&m.state);
    let _ok = m
        .run_unproven(SystemCmd::TransferApp {
            app_id: "c".into(),
            to: "bob".into(),
            actor: "alice".into(),
            consent: serde_json::Value::Null,
            owner_in_org: true,
            owner_in_team: true,
        })
        .unwrap();
    assert_totals_are_sums(&m.state);
    assert_eq!(m.state.file_bytes("account:bob"), 70);
    assert_eq!(m.state.file_bytes("account:alice"), 0);
    let _ok = m
        .run_unproven(SystemCmd::DeleteApp {
            app_id: "a".into(),
            actor: "alice".into(),
            owner_in_org: true,
            owner_in_team: true,
        })
        .unwrap();
    assert_totals_are_sums(&m.state);
    assert!(!m.state.payer_file_bytes.contains_key("org"));
}

#[test]
fn a_personal_app_moves_only_to_an_owner_whose_quota_holds_its_files() {
    let mut m = machine_with_apps(100, &[("a", None), ("b", None)]);
    let _ok = m.run_unproven(genesis(all(Authority::Owner))).unwrap();
    let _ok = m.run_unproven(account("bob")).unwrap();
    let _ok = m.run_unproven(charge("a", 80)).unwrap();
    // Bob holds 50 bytes in an app of his own.
    let mut bobs = m.state.apps["b"].clone();
    bobs.owner = "bob".into();
    let _prev = m.state.apps.insert("b".into(), bobs);
    let _ok = m.run_unproven(charge("b", 50)).unwrap();
    let transfer = SystemCmd::TransferApp {
        app_id: "a".into(),
        to: "bob".into(),
        actor: "alice".into(),
        consent: serde_json::Value::Null,
        owner_in_org: true,
        owner_in_team: true,
    };
    let refused = m.run_unproven(transfer.clone()).unwrap_err();
    assert!(refused.starts_with(QUOTA_PREFIX), "{refused}");
    assert_eq!(m.state.apps["a"].owner, "alice");
    let _ok = m.run_unproven(charge("b", -50)).unwrap();
    let _ok = m.run_unproven(transfer).unwrap();
    assert_eq!(m.state.file_bytes("account:bob"), 80);
}

// ------------------------------------------------------------ package origins

const NOTES: &str = "https://notes.example.com";

fn add_origin(actor: &str, package: &str, origin: &str, rp_id: &str, consent: Value) -> SystemCmd {
    SystemCmd::AddPackageOrigin {
        package: package.into(),
        origin: origin.into(),
        rp_id: rp_id.into(),
        served: false,
        actor: actor.into(),
        at: 1,
        consent,
    }
}

fn add_served(actor: &str, origin: &str, rp_id: &str, consent: Value) -> SystemCmd {
    SystemCmd::AddPackageOrigin {
        package: "p".into(),
        origin: origin.into(),
        rp_id: rp_id.into(),
        served: true,
        actor: actor.into(),
        at: 1,
        consent,
    }
}

fn remove_origin(actor: &str, origin: &str, consent: Value) -> SystemCmd {
    SystemCmd::RemovePackageOrigin {
        package: "p".into(),
        origin: origin.into(),
        actor: actor.into(),
        consent,
    }
}

/// Package `q`, its first release installed by `by`.
fn install_q(by: &str) -> SystemCmd {
    SystemCmd::InstallPackage {
        package: "q".into(),
        signer_id: "did:key:q".into(),
        release: serde_json::from_value(json!({
            "version": "1.0.0", "mpk": "q1", "mpkSize": 1, "source": "upload",
            "services": { "": "b-q" }, "installedBy": by, "installedAt": 1
        }))
        .unwrap(),
    }
}

/// Registering and removing a package's origin applies only with its
/// manager's passkey consent over exactly that change, once; anyone
/// else's, however valid, is refused where it applies.
#[test]
fn package_origins_carry_their_managers_consent() {
    let (mut m, mut alice, mut bob) = two_accounts();
    let _ok = m.run_unproven(install_at("alice", "1.0.0", 1)).unwrap();
    let args = json!({ "origin": NOTES, "rpId": "notes.example.com" });
    let by_bob = consent(&mut bob, "package:p", "__add_origin", &args);
    assert!(
        m.run(add_origin("bob", "p", NOTES, "notes.example.com", by_bob))
            .unwrap_err()
            .contains("manager"),
        "not the package's manager"
    );
    for (why, rp_id, c) in [
        ("no consent", "notes.example.com", Value::Null),
        (
            "over another rpId",
            "example.com",
            consent(&mut alice, "package:p", "__add_origin", &args),
        ),
        (
            "over another package",
            "notes.example.com",
            consent(&mut alice, "package:q", "__add_origin", &args),
        ),
    ] {
        assert!(
            m.run(add_origin("alice", "p", NOTES, rp_id, c)).is_err(),
            "{why}"
        );
    }
    assert!(m.state.package_origins.is_empty());
    let c = consent(&mut alice, "package:p", "__add_origin", &args);
    let _ok = m
        .run(add_origin(
            "alice",
            "p",
            NOTES,
            "notes.example.com",
            c.clone(),
        ))
        .unwrap();
    assert_eq!(m.state.package_origins[NOTES].package, "p");
    assert_eq!(m.state.package_origins[NOTES].rp_id, "notes.example.com");
    assert!(
        m.run(add_origin("alice", "p", NOTES, "notes.example.com", c))
            .unwrap_err()
            .contains("used already"),
        "a consent is spent once"
    );

    let gone = json!({ "origin": NOTES });
    let by_bob = consent(&mut bob, "package:p", "__remove_origin", &gone);
    assert!(m.run(remove_origin("bob", NOTES, by_bob)).is_err());
    let wrong = consent(
        &mut alice,
        "package:p",
        "__remove_origin",
        &json!({ "origin": "https://x.example.com" }),
    );
    assert!(m.run(remove_origin("alice", NOTES, wrong)).is_err());
    assert!(m.state.package_origins.contains_key(NOTES));
    let c = consent(&mut alice, "package:p", "__remove_origin", &gone);
    let _ok = m.run(remove_origin("alice", NOTES, c)).unwrap();
    assert!(m.state.package_origins.is_empty());
}

/// A package's publisher manages its origins once its key is bound: the
/// first installer no longer does; the operator never does.
#[test]
fn a_bound_publisher_manages_its_packages_origins() {
    let (mut m, _alice, _bob) = two_accounts();
    let _ok = m.run_unproven(install_at("alice", "1.0.0", 1)).unwrap();
    assert!(m.state.manages_package("p", "alice"));
    assert!(!m.state.manages_package("p", "bob"));
    assert!(!m.state.manages_package("p", OPERATOR));
    assert!(!m.state.manages_package("nope", "alice"));
    let _ok = m
        .run_unproven(SystemCmd::CreateOrg {
            org_id: "o".into(),
            name: "o".into(),
            actor: "bob".into(),
            release: None,
            recovery_key: None,
            at: 0,
            name_fits: true,
        })
        .unwrap();
    let _prev = m.state.publishers.insert(
        "did:key:z".into(),
        super::PublisherBinding {
            org: "o".into(),
            generation: 1,
        },
    );
    assert!(m.state.manages_package("p", "bob"));
    assert!(!m.state.manages_package("p", "alice"));
}

/// One package per origin; at most `MAX_ORIGINS_PER_PACKAGE` per
/// package; an origin or rp-id that does not fit is refused at apply too;
/// a package uninstalled takes its origins with it.
#[test]
fn package_origins_are_unique_bounded_and_go_with_their_package() {
    use crate::origins::MAX_ORIGINS_PER_PACKAGE;
    let (mut m, _alice, _bob) = two_accounts();
    let _ok = m.run_unproven(install_at("alice", "1.0.0", 1)).unwrap();
    let _ok = m.run_unproven(install_q("bob")).unwrap();
    let add = |m: &mut SystemMachine, who: &str, pkg: &str, origin: &str, rp: &str| {
        m.run_unproven(add_origin(who, pkg, origin, rp, Value::Null))
    };
    let _ok = add(&mut m, "alice", "p", NOTES, "example.com").unwrap();
    // Again, the same: answered as it stands.
    let _ok = add(&mut m, "alice", "p", NOTES, "example.com").unwrap();
    assert!(
        add(&mut m, "alice", "p", NOTES, "notes.example.com").is_err(),
        "another rpId"
    );
    assert!(
        add(&mut m, "bob", "q", NOTES, "notes.example.com")
            .unwrap_err()
            .contains("another package"),
        "first come"
    );
    for (origin, rp) in [
        ("http://notes.example.com", "notes.example.com"),
        ("https://notes.example.com/", "notes.example.com"),
        ("https://x.example.com", "other.com"),
        ("https://x.example.com", "com"),
    ] {
        assert!(
            add(&mut m, "alice", "p", origin, rp).is_err(),
            "{origin} {rp}"
        );
    }
    for i in 1..MAX_ORIGINS_PER_PACKAGE {
        let _ok = add(
            &mut m,
            "alice",
            "p",
            &format!("https://n{i}.example.com"),
            "example.com",
        )
        .unwrap();
    }
    assert!(add(
        &mut m,
        "alice",
        "p",
        "https://more.example.com",
        "example.com"
    )
    .unwrap_err()
    .starts_with(QUOTA_PREFIX));
    let _ok = add(&mut m, "bob", "q", "https://q.example.org", "q.example.org").unwrap();

    let _ok = m
        .run_unproven(SystemCmd::UninstallPackage {
            package: "p".into(),
            version: None,
            actor: "alice".into(),
        })
        .unwrap();
    assert_eq!(
        m.state.package_origins.keys().collect::<Vec<_>>(),
        ["https://q.example.org"],
        "p's origins went with it"
    );
}

/// A served origin (a custom domain) applies only with its manager's
/// consent naming it served: the consent to the same origin unserved,
/// as a client from before signs it, does not make it served. One
/// registration holds an origin either way, at most
/// `MAX_SERVED_ORIGINS_PER_PACKAGE` of a package's are served, and one
/// with a port is refused where it applies.
#[test]
fn served_origins_carry_their_own_consent_and_limit() {
    use crate::origins::{add_args, MAX_SERVED_ORIGINS, MAX_SERVED_ORIGINS_PER_PACKAGE};
    const KV: &str = "https://kv.example.com";
    let (mut m, mut alice, mut bob) = two_accounts();
    let _ok = m.run_unproven(install_at("alice", "1.0.0", 1)).unwrap();
    let plain = consent(
        &mut alice,
        "package:p",
        "__add_origin",
        &add_args(KV, "kv.example.com", false),
    );
    assert!(
        m.run(add_served("alice", KV, "kv.example.com", plain))
            .is_err(),
        "a consent to an unserved origin"
    );
    let args = add_args(KV, "kv.example.com", true);
    let by_bob = consent(&mut bob, "package:p", "__add_origin", &args);
    assert!(m
        .run(add_served("bob", KV, "kv.example.com", by_bob))
        .unwrap_err()
        .contains("manager"));
    assert!(m.state.package_origins.is_empty());
    let c = consent(&mut alice, "package:p", "__add_origin", &args);
    let out = m.run(add_served("alice", KV, "kv.example.com", c)).unwrap();
    assert_eq!(out["served"], true, "{out}");
    assert!(m.state.served_origin(KV).is_some());
    // The served consent does not register it unserved either.
    let c = consent(&mut alice, "package:p", "__add_origin", &args);
    assert!(m
        .run(add_origin("alice", "p", KV, "kv.example.com", c))
        .is_err());

    let add = |m: &mut SystemMachine, origin: &str, served: bool| {
        let mut cmd = add_origin("alice", "p", origin, "example.com", Value::Null);
        if let SystemCmd::AddPackageOrigin { served: s, .. } = &mut cmd {
            *s = served;
        }
        m.run_unproven(cmd)
    };
    assert!(
        add(&mut m, KV, false).is_err(),
        "held served, under another rpId too"
    );
    assert!(
        add(&mut m, "https://kv.example.com:8443", true).is_err(),
        "a port"
    );
    for i in 1..MAX_SERVED_ORIGINS_PER_PACKAGE {
        let _ok = add(&mut m, &format!("https://s{i}.example.com"), true).unwrap();
    }
    assert!(add(&mut m, "https://more.example.com", true)
        .unwrap_err()
        .starts_with(QUOTA_PREFIX));
    let _ok = add(&mut m, "https://plain.example.com", false).unwrap();
    assert!(m.state.served_origin("https://plain.example.com").is_none());

    // The cluster's own bound, whoever holds the rest.
    let _ok = m.run_unproven(install_q("bob")).unwrap();
    let held = m
        .state
        .package_origins
        .values()
        .filter(|o| o.served)
        .count();
    for i in held..MAX_SERVED_ORIGINS {
        let mut o = m.state.package_origins[KV].clone();
        o.package = "elsewhere".into();
        let _prev = m
            .state
            .package_origins
            .insert(format!("https://e{i}.example.org"), o);
    }
    let mut cmd = add_origin(
        "bob",
        "q",
        "https://q.example.org",
        "q.example.org",
        Value::Null,
    );
    if let SystemCmd::AddPackageOrigin { served, .. } = &mut cmd {
        *served = true;
    }
    assert_eq!(
        m.run_unproven(cmd).unwrap_err(),
        "quota exceeded: served origins"
    );
}

/// A registration and an `AddPackageOrigin` from before served origins
/// read as unserved and write the bytes they did.
#[test]
fn origins_from_before_served_ones_read_unserved_and_write_as_before() {
    let record = json!({
        "package": "p", "rpId": "notes.example.com", "addedBy": "alice", "addedAt": 1
    });
    let o: crate::origins::PackageOrigin = serde_json::from_value(record.clone()).unwrap();
    assert!(!o.served);
    assert_eq!(serde_json::to_value(&o).unwrap(), record);
    let cmd = json!({
        "op": "addPackageOrigin", "package": "p", "origin": NOTES,
        "rp_id": "notes.example.com", "actor": "alice", "at": 1, "consent": null
    });
    let read: SystemCmd = serde_json::from_value(cmd.clone()).unwrap();
    assert!(matches!(
        read,
        SystemCmd::AddPackageOrigin { served: false, .. }
    ));
    assert_eq!(serde_json::to_value(&read).unwrap(), cmd);
}

/// State, refresh records and session claims from before package origins
/// read with none and write the bytes they did.
#[test]
fn state_from_before_package_origins_reads_with_none_and_writes_as_before() {
    let mut m = SystemMachine::default();
    let _ok = m.run_authorized(genesis(all(Authority::Owner))).unwrap();
    let _ok = m.run_authorized(account("alice")).unwrap();
    let _ok = m.run_unproven(install_at("alice", "1.0.0", 1)).unwrap();
    let refresh: RefreshRecord = serde_json::from_value(json!({
        "account": "alice", "session": "s", "expiresAt": 9, "passkey": "c", "app": "p"
    }))
    .unwrap();
    assert_eq!(refresh.origin, None);
    let _prev = m.state.refresh.insert("h".into(), refresh);
    let before = serde_json::to_value(&m.state).unwrap();
    assert!(before.get("packageOrigins").is_none(), "{before}");
    assert!(before["refresh"]["h"].get("origin").is_none(), "{before}");
    let restored: SystemState = serde_json::from_value(before.clone()).unwrap();
    assert!(restored.package_origins.is_empty());
    assert_eq!(serde_json::to_value(&restored).unwrap(), before);

    // A token minted before names no origin, and verifies as it did.
    let claims: calimero_identity::SessionClaims = serde_json::from_value(json!({
        "account": "alice", "session": "s", "expires_at": 9, "app": "p", "node": 1, "key": "k"
    }))
    .unwrap();
    assert_eq!(claims.origin, None);
    assert!(serde_json::to_value(&claims)
        .unwrap()
        .get("origin")
        .is_none());
}

/// Invites, teams and default-capability entries from before app teams
/// read with no creator, no team default and the organization's default,
/// and write the bytes they did.
#[test]
fn invites_teams_and_default_caps_from_before_app_teams_read_and_write_as_before() {
    let invite = json!({
        "org": "o", "role": "member", "teams": ["t"], "expiresAt": 9, "usesLeft": 2
    });
    let read: crate::types::Invite = serde_json::from_value(invite.clone()).unwrap();
    assert!(read.created_by.is_empty());
    assert_eq!(read.created_at, 0);
    assert_eq!(serde_json::to_value(&read).unwrap(), invite);

    let team = json!({
        "teamId": "t", "name": "chat", "parent": null, "join": "everyone",
        "members": { "a": true }, "readOnly": [], "capabilities": {},
        "metadata": { "app": { "package": "com.chat" } }, "removed": []
    });
    let read: crate::types::Team = serde_json::from_value(team.clone()).unwrap();
    assert_eq!(read.default_capabilities, None);
    assert_eq!(read.app_package(), Some("com.chat"));
    assert_eq!(serde_json::to_value(&read).unwrap(), team);

    let cmd = json!({
        "op": "setDefaultCapabilities", "org": "o", "capabilities": 4,
        "actor": "a", "consent": null
    });
    let read: SystemCmd = serde_json::from_value(cmd.clone()).unwrap();
    assert!(matches!(
        read,
        SystemCmd::SetDefaultCapabilities { team: None, .. }
    ));
    assert_eq!(serde_json::to_value(&read).unwrap(), cmd);
    // Its consent states what it did before.
    assert_eq!(
        super::default_capabilities_args(None, 4),
        json!({ "capabilities": 4 })
    );
}

#[test]
fn a_scope_apps_from_before_a_per_app_join_reads_writes_and_consents_as_before() {
    let cmd = json!({
        "op": "scopeApps", "org": "o", "apps": [{ "package": "com.chat" }],
        "shared": true, "nonce": "n", "actor": "a", "consent": null
    });
    let read: SystemCmd = serde_json::from_value(cmd.clone()).unwrap();
    let SystemCmd::ScopeApps { apps, shared, .. } = &read else {
        panic!("{read:?}");
    };
    let apps = apps.as_deref().unwrap();
    assert_eq!((apps[0].join, apps[0].keep.as_ref()), (None, None));
    assert_eq!(serde_json::to_value(&read).unwrap(), cmd);
    assert_eq!(
        super::scope_apps_args(Some(apps), *shared),
        json!({ "apps": [{ "package": "com.chat" }], "shared": true })
    );
}

#[test]
fn state_with_no_compute_use_writes_no_compute_keys() {
    let mut m = SystemMachine::default();
    let _ok = m.run_authorized(genesis(all(Authority::Owner))).unwrap();
    let before = serde_json::to_value(&m.state).unwrap();
    for key in ["compute", "nextRunSeq", "runGrants", "releasedRuns"] {
        assert!(before.get(key).is_none(), "{key} in {before}");
    }
}

// ------------------------------------------------------------ recovery

use super::{
    NO_RECOVERY_KEY, NO_RECOVERY_PENDING, OWNER_ONLY_RECOVERY, RECOVERY_NOT_READY, RECOVERY_PENDING,
};

const SEED: [u8; 32] = [9; 32];

fn set_key(key: Option<String>, actor: &str, consent: serde_json::Value) -> SystemCmd {
    SystemCmd::SetRecoveryKey {
        org: "org".into(),
        public_key: key,
        at: 10,
        actor: actor.into(),
        consent,
    }
}

fn start(seed: &[u8; 32], account: &str, nonce: &str, at: u64) -> SystemCmd {
    start_signed_for(seed, account, account, nonce, at)
}

/// A start for `account` carrying a signature the seed made for `signed_for`.
fn start_signed_for(
    seed: &[u8; 32],
    account: &str,
    signed_for: &str,
    nonce: &str,
    at: u64,
) -> SystemCmd {
    SystemCmd::StartRecovery {
        org: "org".into(),
        account: account.into(),
        nonce: nonce.into(),
        signature: calimero_identity::recovery::sign_recovery(seed, "org", signed_for, nonce),
        at,
    }
}

fn public(seed: &[u8; 32]) -> String {
    calimero_identity::recovery::recovery_public(seed)
}

/// The owner (and only the owner) sets the key with a passkey; then the
/// seed, and nothing else, starts a recovery for the account it signed
/// for; the owner cancels with a passkey; the recovering account finishes
/// with its own passkey once the delay passed, and takes the
/// organization and the previous owner's apps in it.
#[test]
fn recovery_needs_the_owners_passkey_the_seed_and_the_delay() {
    let (mut m, mut alice, mut bob, mut carol) = proven_cluster();
    let _ = m
        .state
        .apps
        .get_mut("app")
        .map(|a| a.org = Some("org".into()));
    let key = Some(public(&SEED));
    let args = json!({ "publicKey": key });

    // No key yet: the seed starts nothing.
    assert_eq!(
        m.run(start(&SEED, "carol", "n0", 100)).unwrap_err(),
        NO_RECOVERY_KEY
    );
    // A member's own passkey does not set it.
    let by_bob = consent(&mut bob, "org:org", "__set_recovery_key", &args);
    assert_eq!(
        m.run(set_key(key.clone(), "bob", by_bob)).unwrap_err(),
        OWNER_ONLY_RECOVERY
    );
    needs_consent(
        &mut m,
        &mut alice,
        ("org:org", "__set_recovery_key"),
        &args,
        &json!({ "publicKey": public(&[1; 32]) }),
        |c| set_key(key.clone(), "alice", c),
    );
    assert_eq!(m.state.orgs["org"].recovery.as_ref().unwrap().set_at, 10);

    // A wrong seed, a signature for another account, and another org's
    // signature are refused where the command applies.
    assert!(m.run(start(&[1; 32], "carol", "n1", 100)).is_err());
    assert!(m
        .run(start_signed_for(&SEED, "carol", "bob", "n1", 100))
        .is_err());
    let mut other_org = start(&SEED, "carol", "n1", 100);
    if let SystemCmd::StartRecovery { org, .. } = &mut other_org {
        *org = "elsewhere".into();
    }
    assert!(m.run(other_org).is_err());

    // The seed's signature for carol: pending, and only one at a time.
    let out = m.run(start(&SEED, "carol", "n1", 100)).unwrap();
    assert_eq!(
        out["readyAt"],
        json!(100 + crate::types::RECOVERY_DELAY_SECS)
    );
    assert_eq!(
        m.run(start(&SEED, "bob", "n2", 101)).unwrap_err(),
        RECOVERY_PENDING
    );

    // The owner cancels; the same signature cannot start it again.
    let cancel = |actor: &str, c| SystemCmd::CancelRecovery {
        org: "org".into(),
        account: "carol".into(),
        actor: actor.into(),
        consent: c,
    };
    let by_bob = consent(
        &mut bob,
        "org:org",
        "__cancel_recovery",
        &json!({ "account": "carol" }),
    );
    assert_eq!(
        m.run(cancel("bob", by_bob)).unwrap_err(),
        OWNER_ONLY_RECOVERY
    );
    let by_alice = consent(
        &mut alice,
        "org:org",
        "__cancel_recovery",
        &json!({ "account": "carol" }),
    );
    let _ok = m.run(cancel("alice", by_alice)).unwrap();
    assert_eq!(
        m.run(start(&SEED, "carol", "n1", 100)).unwrap_err(),
        "that recovery signature was used already"
    );

    // Started again; finished only by carol, with her passkey, after the delay.
    let _ok = m.run(start(&SEED, "carol", "n3", 200)).unwrap();
    let ready = 200 + crate::types::RECOVERY_DELAY_SECS;
    let finish = |actor: &str, at: u64, c| SystemCmd::FinishRecovery {
        org: "org".into(),
        at,
        actor: actor.into(),
        consent: c,
    };
    let args = json!({ "org": "org" });
    let early = consent(&mut carol, "org:org", "__finish_recovery", &args);
    assert_eq!(
        m.run(finish("carol", ready - 1, early)).unwrap_err(),
        RECOVERY_NOT_READY
    );
    let by_bob = consent(&mut bob, "org:org", "__finish_recovery", &args);
    assert_eq!(
        m.run(finish("bob", ready, by_bob)).unwrap_err(),
        NO_RECOVERY_PENDING
    );
    let by_alice_for_carol = consent(&mut alice, "org:org", "__finish_recovery", &args);
    assert!(m.run(finish("carol", ready, by_alice_for_carol)).is_err());
    let done = m
        .run(finish(
            "carol",
            ready,
            consent(&mut carol, "org:org", "__finish_recovery", &args),
        ))
        .unwrap();
    assert_eq!(done["previousOwners"], json!(["alice"]));
    let o = &m.state.orgs["org"];
    assert_eq!(o.role("carol"), Some(OrgRole::Owner));
    assert_eq!(o.role("alice"), None, "the previous owner left");
    assert_eq!(o.role("bob"), Some(OrgRole::Member));
    assert!(o.recovery.as_ref().unwrap().pending.is_none());
    assert_eq!(
        m.state.apps["app"].owner, "carol",
        "the org's apps moved with it"
    );
    // Alice's passkeys and account are her own still.
    assert!(m.state.accounts.contains_key("alice"));
}

/// Rotating the key cancels a pending recovery and retires the old seed;
/// removing it leaves nothing to recover with.
#[test]
fn rotating_the_recovery_key_retires_the_old_seed() {
    let (mut m, mut alice, _bob, _carol) = proven_cluster();
    let set = |m: &mut SystemMachine, alice: &mut Soft, key: Option<String>| {
        let c = consent(
            alice,
            "org:org",
            "__set_recovery_key",
            &json!({ "publicKey": key }),
        );
        m.run(set_key(key, "alice", c)).unwrap();
    };
    set(&mut m, &mut alice, Some(public(&SEED)));
    let _ok = m.run(start(&SEED, "carol", "n1", 100)).unwrap();
    set(&mut m, &mut alice, Some(public(&[2; 32])));
    assert!(m.state.orgs["org"]
        .recovery
        .as_ref()
        .unwrap()
        .pending
        .is_none());
    assert!(
        m.run(start(&SEED, "carol", "n2", 100)).is_err(),
        "the old seed"
    );
    let _ok = m.run(start(&[2; 32], "carol", "n2", 100)).unwrap();
    set(&mut m, &mut alice, None);
    assert!(m.state.orgs["org"].recovery.is_none());
    assert_eq!(
        m.run(start(&[2; 32], "carol", "n3", 100)).unwrap_err(),
        NO_RECOVERY_KEY
    );
    // A key that is not an Ed25519 public key is refused.
    let bad = Some("not-a-key".to_owned());
    let c = consent(
        &mut alice,
        "org:org",
        "__set_recovery_key",
        &json!({ "publicKey": bad }),
    );
    assert!(m.run(set_key(bad, "alice", c)).is_err());
}

/// An organization written before recovery keys reads with none and
/// writes back the same bytes, so a snapshot from before restores
/// unchanged; its owner can set a key afterwards.
#[test]
fn state_from_before_recovery_reads_with_none_and_writes_as_before() {
    let written = json!({
        "orgId": "org", "name": "Docs", "members": { "alice": "owner" }, "teams": {},
        "defaultCapabilities": crate::types::caps::DEFAULT, "capabilities": {},
        "approvedReleases": ["2.0.0"], "outboundApprovals": {}, "metadata": null,
        "memberMetadata": {}, "upgradeRuns": {},
    });
    let org: crate::types::Org = serde_json::from_value(written.clone()).unwrap();
    assert!(org.recovery.is_none());
    assert_eq!(serde_json::to_value(&org).unwrap(), written);
    // `CreateOrg` as the previous release logged it.
    let cmd: SystemCmd = serde_json::from_value(json!({
        "op": "createOrg", "org_id": "o2", "name": "N", "actor": "alice", "release": null,
    }))
    .unwrap();
    assert!(matches!(
        cmd,
        SystemCmd::CreateOrg {
            recovery_key: None,
            at: 0,
            ..
        }
    ));

    let (mut m, mut alice, _bob, _carol) = proven_cluster();
    let _prev = m.state.orgs.insert("org".into(), org);
    let before = serde_json::to_value(&m.state).unwrap();
    let restored: SystemState = serde_json::from_value(before.clone()).unwrap();
    assert_eq!(serde_json::to_value(&restored).unwrap(), before);
    let key = Some(public(&SEED));
    let c = consent(
        &mut alice,
        "org:org",
        "__set_recovery_key",
        &json!({ "publicKey": key }),
    );
    let _ok = m.run(set_key(key.clone(), "alice", c)).unwrap();
    assert_eq!(
        m.state.orgs["org"].recovery.as_ref().map(|r| &r.public_key),
        key.as_ref()
    );
}

mod sign_up {
    use serde_json::json;

    use super::super::{
        Governance, SignUp, SignUpInvite, SignUpRedemption, SystemCmd, SystemMachine, SystemState,
        OPERATOR, SIGN_UP_PREFIX,
    };
    use super::{account_with, operator_signed};

    fn machine(policy: SignUp) -> SystemMachine {
        let key = ed25519_dalek::SigningKey::from_bytes(&[9; 32]);
        let mut m = SystemMachine::default();
        let _ok = m
            .run(SystemCmd::Genesis {
                governance: Governance {
                    sign_up: policy,
                    ..Governance::default()
                },
                release: Some("1.0.0".into()),
                operator_key: Some(crate::proof::OperatorKey {
                    key: hex::encode(key.verifying_key().to_bytes()),
                    cluster: "c".into(),
                }),
                nodes: Vec::new(),
                session_proofs: true,
            })
            .unwrap();
        m
    }

    /// `id`'s account with its passkey's consent, presenting `token`
    /// (signed for `id` at `now`).
    fn signing_up(id: &str, token: Option<&str>, now: u64) -> SystemCmd {
        let mut pk = calimero_identity::testing::SoftAuthenticator::new(
            "localhost",
            "http://localhost:3000",
        );
        let SystemCmd::CreateAccount {
            account,
            refresh,
            consent,
            ..
        } = account_with(id, &mut pk)
        else {
            unreachable!()
        };
        SystemCmd::CreateAccount {
            at: 0,
            account,
            invite: token.map(|t| SignUpRedemption {
                invite_key: crate::proof::invite_key(t),
                now,
                signature: crate::proof::sign_invite_redeem(t, id, now),
            }),
            refresh,
            consent,
        }
    }

    fn sign_up_invite(token: &str, uses: u32, expires_at: u64) -> SystemCmd {
        let invite = SignUpInvite {
            expires_at,
            uses_left: uses,
            created_by: OPERATOR.into(),
            created_at: 1,
        };
        let key = crate::proof::invite_key(token);
        let args = super::super::sign_up_invite_args(&key, &invite);
        SystemCmd::CreateSignUpInvite {
            invite_key: key,
            invite,
            actor: OPERATOR.into(),
            proof: Some(operator_signed("cluster", "__create_sign_up_invite", &args)),
            consent: None,
        }
    }

    /// Alice (an account from before, whatever the policy) owns `org`,
    /// which has a one-use member invite under `token`.
    fn org_invite(m: &mut SystemMachine, token: &str) {
        let _prev = m.state.accounts.insert(
            "alice".into(),
            serde_json::from_value(
                json!({ "accountId": "alice", "name": "a", "createdAt": 0, "passkeys": [] }),
            )
            .unwrap(),
        );
        let _prev = m.state.orgs.insert(
            "org".into(),
            serde_json::from_value(json!({
                "orgId": "org", "name": "Docs", "members": { "alice": "owner" }, "teams": {}
            }))
            .unwrap(),
        );
        let _prev = m.state.invites.insert(
            crate::proof::invite_key(token),
            serde_json::from_value(json!({
                "org": "org", "role": "member", "teams": [], "expiresAt": 100, "usesLeft": 1,
            }))
            .unwrap(),
        );
    }

    fn refused(r: Result<serde_json::Value, String>) -> String {
        let e = r.unwrap_err();
        assert!(e.starts_with(SIGN_UP_PREFIX), "{e}");
        e
    }

    /// Open takes anyone, as before; an invite presented there is still
    /// checked and, for an organization's, joins it.
    #[test]
    fn an_open_cluster_takes_anyone() {
        let mut m = machine(SignUp::Open);
        let _ok = m.run(signing_up("a", None, 10)).unwrap();
        let _e = refused(m.run(signing_up("b", Some("inv_unknown"), 10)));
        assert!(!m.state.accounts.contains_key("b"));
        org_invite(&mut m, "inv_org");
        let v = m.run(signing_up("c", Some("inv_org"), 10)).unwrap();
        assert_eq!(v["org"], "org");
        assert!(m.state.orgs["org"].members.contains_key("c"));
    }

    /// Invite: refused without one, with an unknown, expired or used
    /// one, or a forged signature; taken with an organization invite (the
    /// account joins) and with a sign-up invite (spent per account).
    #[test]
    fn an_invite_cluster_takes_only_invited_people() {
        let mut m = machine(SignUp::Invite);
        let e = refused(m.run(signing_up("a", None, 10)));
        assert!(e.contains("invite only"), "{e}");
        let _e = refused(m.run(signing_up("a", Some("inv_unknown"), 10)));

        org_invite(&mut m, "inv_org");
        let _e = refused(m.run(signing_up("a", Some("inv_org"), 100)));
        let SystemCmd::CreateAccount {
            account,
            refresh,
            consent,
            ..
        } = signing_up("a", None, 10)
        else {
            unreachable!()
        };
        let forged = SystemCmd::CreateAccount {
            at: 0,
            account,
            invite: Some(SignUpRedemption {
                invite_key: crate::proof::invite_key("inv_org"),
                now: 10,
                signature: crate::proof::sign_invite_redeem("inv_guess", "a", 10),
            }),
            refresh,
            consent,
        };
        assert!(m.run(forged).is_err(), "not the token's key");
        let _ok = m.run(signing_up("a", Some("inv_org"), 10)).unwrap();
        assert_eq!(
            m.state.orgs["org"].members["a"],
            crate::types::OrgRole::Member
        );
        let _e = refused(m.run(signing_up("b", Some("inv_org"), 10)));

        let _ok = m.run(sign_up_invite("sup_two", 2, 50)).unwrap();
        let _ok = m.run(signing_up("b", Some("sup_two"), 10)).unwrap();
        assert_eq!(
            m.state.sign_up_invites[&crate::proof::invite_key("sup_two")].uses_left,
            1
        );
        let _e = refused(m.run(signing_up("c", Some("sup_two"), 50)));
        let _ok = m.run(signing_up("c", Some("sup_two"), 11)).unwrap();
        assert!(
            m.state.sign_up_invites.is_empty(),
            "the last use removes it"
        );
        let _e = refused(m.run(signing_up("d", Some("sup_two"), 12)));
        assert_eq!(m.state.accounts.len(), 4);
    }

    /// Closed: only a sign-up invite (a pre-authorization) lets anyone
    /// in; an organization invite does not.
    #[test]
    fn a_closed_cluster_takes_only_preauthorized_people() {
        let mut m = machine(SignUp::Closed);
        let e = refused(m.run(signing_up("a", None, 10)));
        assert!(e.contains("pre-authorized"), "{e}");
        org_invite(&mut m, "inv_org");
        let _e = refused(m.run(signing_up("a", Some("inv_org"), 10)));
        assert_eq!(m.state.invites.len(), 1, "a refusal spends nothing");
        let _ok = m.run(sign_up_invite("sup_one", 1, 50)).unwrap();
        let _ok = m.run(signing_up("a", Some("sup_one"), 10)).unwrap();
        let _e = refused(m.run(signing_up("b", Some("sup_one"), 10)));
    }

    /// The policy and sign-up invites change only with the operator's
    /// signature over exactly that change, once; with the cluster's owner
    /// named (`deployers: owner`), only with the owner's passkey.
    #[test]
    fn the_policy_changes_only_with_the_operators_signature() {
        let mut m = machine(SignUp::Open);
        let set = |policy: SignUp, proof: Option<serde_json::Value>| SystemCmd::SetSignUp {
            policy,
            actor: OPERATOR.into(),
            proof,
            consent: None,
        };
        let closed = json!({ "policy": "closed" });
        assert!(m.run(set(SignUp::Closed, None)).is_err(), "no signature");
        let wrong = operator_signed("cluster", "__sign_up", &json!({ "policy": "invite" }));
        assert!(
            m.run(set(SignUp::Closed, Some(wrong))).is_err(),
            "another change"
        );
        let mut forged = operator_signed("cluster", "__sign_up", &closed);
        forged["signature"] = json!("00".repeat(64));
        assert!(m.run(set(SignUp::Closed, Some(forged))).is_err(), "forged");
        let as_alice = SystemCmd::SetSignUp {
            policy: SignUp::Closed,
            actor: "alice".into(),
            proof: Some(operator_signed("cluster", "__sign_up", &closed)),
            consent: None,
        };
        assert!(m.run(as_alice).is_err(), "only the operator signs");
        assert_eq!(m.state.governance().sign_up, SignUp::Open);
        let proof = operator_signed("cluster", "__sign_up", &closed);
        let _ok = m.run(set(SignUp::Closed, Some(proof.clone()))).unwrap();
        assert_eq!(m.state.governance().sign_up, SignUp::Closed);
        assert!(m.run(set(SignUp::Closed, Some(proof))).is_err(), "replay");

        // A sign-up invite and its revocation: signed, once.
        let SystemCmd::CreateSignUpInvite {
            invite_key, invite, ..
        } = sign_up_invite("sup_x", 1, 50)
        else {
            unreachable!()
        };
        let unsigned = SystemCmd::CreateSignUpInvite {
            invite_key,
            invite,
            actor: OPERATOR.into(),
            proof: None,
            consent: None,
        };
        assert!(m.run(unsigned).is_err(), "no signature");
        let _ok = m.run(sign_up_invite("sup_x", 1, 50)).unwrap();
        let key = crate::proof::invite_key("sup_x");
        let revoke = |proof| SystemCmd::RevokeSignUpInvite {
            invite_key: key.clone(),
            actor: OPERATOR.into(),
            proof,
            consent: None,
        };
        assert!(m.run(revoke(None)).is_err());
        let args = json!({ "inviteKey": key });
        let _ok = m
            .run(revoke(Some(operator_signed(
                "cluster",
                "__revoke_sign_up_invite",
                &args,
            ))))
            .unwrap();
        let _e = refused(m.run(signing_up("a", Some("sup_x"), 10)));

        // Once the cluster's owner is named, the operator's signature no
        // longer changes the policy.
        m.state.governance.as_mut().unwrap().deployers = super::super::Deployers::Owner;
        let _e = refused(m.run(signing_up("owner", None, 10)));
        let _prev = m.state.accounts.insert(
            "owner".into(),
            serde_json::from_value(
                json!({ "accountId": "owner", "name": "o", "createdAt": 0, "passkeys": [] }),
            )
            .unwrap(),
        );
        m.state.cluster_owner = Some(super::super::Deployer::Account("owner".into()));
        let open = json!({ "policy": "open" });
        let e = m
            .run(set(
                SignUp::Open,
                Some(operator_signed("cluster", "__sign_up", &open)),
            ))
            .unwrap_err();
        assert!(e.contains("owner"), "{e}");
        assert!(m.state.runs_sign_up("owner").is_ok());
        assert!(m.state.runs_sign_up(OPERATOR).is_err());
    }

    /// State from before sign-up policies reads as `open` with no
    /// sign-up invites, and writes the bytes it did; a `CreateAccount`
    /// from before (no `invite`) applies as before.
    #[test]
    fn state_from_before_sign_up_reads_as_open() {
        let mut m = SystemMachine::default();
        let _ok = m
            .run_authorized(super::genesis(Governance::default()))
            .unwrap();
        let old = serde_json::to_value(&m.state).unwrap();
        assert!(old.get("signUpInvites").is_none(), "{old}");
        assert!(old["governance"].get("signUp").is_none(), "{old}");
        let mut restored: SystemState = serde_json::from_value(old.clone()).unwrap();
        assert_eq!(restored.governance().sign_up, SignUp::Open);
        assert!(restored.sign_up_invites.is_empty());
        assert_eq!(serde_json::to_value(&restored).unwrap(), old);
        // A command the previous release logged.
        let cmd: SystemCmd = serde_json::from_value(json!({
            "op": "createAccount",
            "account": { "accountId": "a", "name": "a", "createdAt": 0, "passkeys": [] },
        }))
        .unwrap();
        let mut m = SystemMachine {
            state: restored.clone(),
            ..SystemMachine::default()
        };
        let _ok = m.run_authorized(cmd).unwrap();
        restored = m.state;
        assert!(restored.accounts.contains_key("a"));
    }
}

// ---------------------------------------------------- backup keys

use super::{BackupHolder, BAD_BACKUP_KEY, NOT_BACKUP_HOLDER};

fn refused(m: &mut SystemMachine, cmd: SystemCmd) -> String {
    m.run(cmd).unwrap_err()
}

fn set_backup_key(holder: BackupHolder, public_key: &str, actor: &str, proof: Value) -> SystemCmd {
    SystemCmd::SetBackupKey {
        holder,
        public_key: public_key.into(),
        actor: actor.into(),
        at: 5,
        proof,
    }
}

/// Where owners upgrade apps, each app's backups open with its owner's key:
/// an organization's, set by those who upgrade all its apps, or a personal
/// app's owner's, set by that account; each with their own passkey over the
/// key, once. The operator sets none.
#[test]
fn backup_keys_are_set_by_whoever_upgrades_the_apps() {
    let (mut m, mut alice, mut bob, _carol) = proven_cluster();
    let mut org_app = m.state.apps["app"].clone();
    org_app.app_id = "oa".into();
    org_app.org = Some("org".into());
    let _prev = m.state.apps.insert("oa".into(), org_app);
    assert_eq!(
        m.state.backup_holder(&m.state.apps["app"]),
        BackupHolder::Account("alice".into())
    );
    assert_eq!(
        m.state.backup_holder(&m.state.apps["oa"]),
        BackupHolder::Org("org".into())
    );
    let key = "ab".repeat(32);
    let args = json!({ "publicKey": key });
    needs_consent(
        &mut m,
        &mut alice,
        ("account:alice", "__set_backup_key"),
        &args,
        &json!({ "publicKey": "cd".repeat(32) }),
        |c| set_backup_key(BackupHolder::Account("alice".into()), &key, "alice", c),
    );
    assert_eq!(m.state.backup_keys["account:alice"].public_key, key);
    assert_eq!(m.state.backup_keys["account:alice"].set_at, 5);
    // Bob's passkey sets neither alice's key nor the organization's, where
    // he is a member without MANAGE_APPLICATION.
    let c = consent(&mut bob, "account:alice", "__set_backup_key", &args);
    assert_eq!(
        refused(
            &mut m,
            set_backup_key(BackupHolder::Account("alice".into()), &key, "bob", c)
        ),
        NOT_BACKUP_HOLDER
    );
    let c = consent(&mut bob, "org:org", "__set_backup_key", &args);
    assert_eq!(
        refused(
            &mut m,
            set_backup_key(BackupHolder::Org("org".into()), &key, "bob", c)
        ),
        NOT_BACKUP_HOLDER
    );
    // The organization's owner upgrades its apps, so sets its key.
    let c = consent(&mut alice, "org:org", "__set_backup_key", &args);
    let _ok = m
        .run(set_backup_key(
            BackupHolder::Org("org".into()),
            &key,
            "alice",
            c,
        ))
        .unwrap();
    assert_eq!(m.state.backup_keys["org:org"].set_by, "alice");
    // A key that is not 32 bytes of lowercase hex, or all zeros, is refused.
    for bad in ["ab".repeat(31), "AB".repeat(32), "0".repeat(64)] {
        let c = consent(
            &mut alice,
            "account:alice",
            "__set_backup_key",
            &json!({ "publicKey": bad }),
        );
        assert_eq!(
            refused(
                &mut m,
                set_backup_key(BackupHolder::Account("alice".into()), &bad, "alice", c)
            ),
            BAD_BACKUP_KEY
        );
    }
    // The operator does not upgrade apps here, so holds no backup key.
    let proof = signed("operator", "__set_backup_key", &args);
    assert_eq!(
        refused(
            &mut m,
            set_backup_key(BackupHolder::Operator, &key, OPERATOR, proof)
        ),
        NOT_BACKUP_HOLDER
    );
}

/// Where the operator upgrades apps, every app's backups open with the
/// operator's key, set with its signature over the key, once; no owner
/// sets one.
#[test]
fn where_the_operator_upgrades_apps_it_holds_the_backup_key() {
    let mut m = SystemMachine::default();
    let _ok = m
        .run(SystemCmd::Genesis {
            session_proofs: true,
            governance: all(Authority::Operator),
            release: Some("2.0.0".into()),
            operator_key: Some(operator_signer().1),
            nodes: vec![],
        })
        .unwrap();
    let mut alice = Soft::new("localhost", "http://localhost:3000");
    let _ok = m.run(account_with("alice", &mut alice)).unwrap();
    let app: crate::types::AppRecord = serde_json::from_value(json!({
        "appId": "app", "name": "app", "group": 2, "bundle": "b1",
        "owner": "alice", "replicas": [1], "createdAt": 0
    }))
    .unwrap();
    assert_eq!(m.state.backup_holder(&app), BackupHolder::Operator);
    let key = "ab".repeat(32);
    needs_signature(
        &mut m,
        ("operator", "__set_backup_key"),
        &json!({ "publicKey": key }),
        &json!({ "publicKey": "cd".repeat(32) }),
        |p| set_backup_key(BackupHolder::Operator, &key, OPERATOR, p),
    );
    assert_eq!(m.state.backup_keys["operator"].public_key, key);
    let c = consent(
        &mut alice,
        "account:alice",
        "__set_backup_key",
        &json!({ "publicKey": key }),
    );
    assert_eq!(
        refused(
            &mut m,
            set_backup_key(BackupHolder::Account("alice".into()), &key, "alice", c)
        ),
        NOT_BACKUP_HOLDER
    );
}

/// An organization's slice comes in whole, admitted by the operator's
/// signature over exactly its files, and only where nothing of it is here:
/// its org, people (with their passkeys, which sign in as them) and apps, each
/// app waiting in a new group for its file on the seed; its backup key is the
/// one its files were sealed to.
#[test]
fn an_organizations_slice_is_imported_once_with_the_operators_signature() {
    use super::{import_holder_args, ImportedApp, ALREADY_HERE};
    use crate::types::RestoreSource;
    let (mut old, _alice, _bob, _carol) = proven_cluster();
    let mut org_app = old.state.apps["app"].clone();
    org_app.app_id = "oa".into();
    org_app.org = Some("org".into());
    let _prev = old.state.apps.insert("oa".into(), org_app);
    let _prev = old.state.bundles.insert(
        "b1".into(),
        super::BundleRecord {
            hash: "b1".into(),
            size: 1,
            uploaded_by: "alice".into(),
            package: None,
        },
    );
    let holder = BackupHolder::Org("org".into());
    let slice = old.state.holder_slice(&holder);
    assert_eq!(slice.apps.keys().collect::<Vec<_>>(), ["oa"]);
    assert_eq!(slice.accounts.len(), 2, "the org's members: alice and bob");

    // A cluster that never held them.
    let mut m = SystemMachine::default();
    let _ok = m
        .run(SystemCmd::Genesis {
            session_proofs: true,
            governance: all(Authority::Owner),
            release: Some("2.0.0".into()),
            operator_key: Some(operator_signer().1),
            nodes: [1, 2, 3]
                .map(|id| crate::types::NodeRecord {
                    id,
                    addr: format!("http://n{id}/"),
                    zone: String::new(),
                    role: crate::types::NodeRole::Replica,
                    pool: String::new(),
                })
                .to_vec(),
        })
        .unwrap();
    let apps = vec![ImportedApp {
        app_id: "oa".into(),
        replicas: vec![2, 1, 3],
        source: RestoreSource {
            name: "app-oa.rbak".into(),
            sha256: "ab".repeat(32),
            index: 9,
        },
    }];
    let import = |proof: Value| SystemCmd::ImportHolder {
        holder: holder.clone(),
        slice: Box::new(slice.clone()),
        slice_sha256: "cd".repeat(32),
        public_key: "ef".repeat(32),
        seed: 2,
        apps: apps.clone(),
        at: 7,
        proof,
    };
    let args = import_holder_args(&holder, &"cd".repeat(32), &apps);
    needs_signature(
        &mut m,
        ("cluster", "__recover_holder"),
        &args,
        &json!({ "holder": "org:other" }),
        import,
    );
    let app = &m.state.apps["oa"];
    assert_eq!(app.replicas, [2, 1, 3]);
    let restore = app.restore.as_ref().unwrap();
    assert_eq!(restore.seed, Some(2));
    assert_eq!(restore.source.as_ref().unwrap().index, 9);
    assert!(m.state.orgs.contains_key("org"));
    let alice_key = &old.state.accounts["alice"].passkeys[0].id;
    assert_eq!(
        m.state
            .account_of_credential(alice_key)
            .map(|(a, _)| a.account_id.as_str()),
        Some("alice"),
        "she signs in with the passkey she had"
    );
    assert_eq!(m.state.backup_keys["org:org"].public_key, "ef".repeat(32));
    // Once: the organization is here now.
    let proof = signed("cluster", "__recover_holder", &args);
    assert_eq!(refused(&mut m, import(proof)), ALREADY_HERE);
}

/// Setting an organization's backup key is judged on the state when it
/// applies: a member granted MANAGE_APPLICATION org-wide may, and once
/// removed from the organization may not, whatever they held before.
#[test]
fn a_removed_member_no_longer_sets_the_organizations_backup_key() {
    let (mut m, _alice, mut bob, _carol) = proven_cluster();
    let _prev = m
        .state
        .orgs
        .get_mut("org")
        .unwrap()
        .capabilities
        .insert("bob".into(), crate::types::caps::MANAGE_APPLICATION);
    let holder = BackupHolder::Org("org".into());
    let key = "ab".repeat(32);
    let c = consent(
        &mut bob,
        "org:org",
        "__set_backup_key",
        &json!({ "publicKey": key }),
    );
    let _ok = m
        .run(set_backup_key(holder.clone(), &key, "bob", c))
        .unwrap();
    assert_eq!(m.state.backup_keys["org:org"].set_by, "bob");
    let _gone = m.state.orgs.get_mut("org").unwrap().members.remove("bob");
    let other = "cd".repeat(32);
    let c = consent(
        &mut bob,
        "org:org",
        "__set_backup_key",
        &json!({ "publicKey": other }),
    );
    assert_eq!(
        refused(&mut m, set_backup_key(holder, &other, "bob", c)),
        NOT_BACKUP_HOLDER
    );
    assert_eq!(m.state.backup_keys["org:org"].public_key, key);
}
