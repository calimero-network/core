use serde_json::{json, Value};

use super::{
    Authority, Governance, OrgRole, PackageRef, Quotas, ReleaseScope, SystemCmd, SystemMachine,
    SystemState, INVITE_GONE, OPERATOR, SHIPS_IN_OTHER_PACKAGE,
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
        personal_org: Some(super::PersonalOrgSeed {
            org_id: p(id),
            release: None,
        }),
    }
}

/// `owner`'s personal organization in these tests, made with the account.
fn p(owner: &str) -> String {
    format!("p-{owner}")
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
            "org": org.unwrap_or("p-alice"), "grants": access, "replicas": [1, 2, 3], "createdAt": 0
        }))
        .unwrap()
    };
    for (id, a) in [
        (
            "in-org",
            app("in-org", Some("org"), json!([{ "kind": "org" }])),
        ),
        (
            "in-team",
            app(
                "in-team",
                Some("org"),
                json!([{ "kind": "team", "team": "t" }]),
            ),
        ),
        ("personal", app("personal", None, json!([]))),
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
        in_deployment: false,
        owner_approves: false,
        class: None,
        auto: false,
        members_stay: false,
        restore_point: None,
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
    if let Some(o) = m.state.orgs.get_mut(&p("alice")) {
        let _new = o.approved_releases.insert("2.0.0".into());
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
            "owner": "alice", "org": p("alice"), "replicas": [1, 2, 3], "createdAt": 0
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
        in_deployment: false,
        owner_approves: false,
        class: None,
        auto: false,
        members_stay: false,
        restore_point: None,
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
        scope: ReleaseScope::Org { org: p("alice") },
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
        .run_authorized(outbound(ReleaseScope::Org { org: p("alice") }, "alice"))
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

/// A private release is used by the members of the organizations it was installed for,
/// each install for another adding it, and by no one else, an installer or a reader of an
/// instance running it included; a public one by everyone, never private again.
#[test]
fn a_private_release_is_used_only_in_the_organizations_it_was_installed_for() {
    use super::{Visibility, NOT_AN_ORG_MEMBER};
    let mut m = SystemMachine::default();
    let _ok = m.run_authorized(genesis(all(Authority::Owner))).unwrap();
    for who in ["alice", "bob", "carol", "dave"] {
        let _ok = m.run_authorized(account(who)).unwrap();
    }
    // A (alice's) holds carol and dave as a guest; B (bob's) holds alice.
    let join = |m: &mut SystemMachine, org: &str, who: &str, role: OrgRole| {
        let _prev = m
            .state
            .orgs
            .get_mut(&p(org))
            .unwrap()
            .members
            .insert(who.into(), role);
    };
    join(&mut m, "alice", "carol", OrgRole::Member);
    join(&mut m, "alice", "dave", OrgRole::Guest);
    join(&mut m, "bob", "alice", OrgRole::Member);
    let install = |by: &str, orgs: &[&str], visibility: &str| SystemCmd::InstallPackage {
        package: "p".into(),
        signer_id: "did:key:z".into(),
        release: serde_json::from_value(json!({
            "version": "1.0.0", "mpk": "ab", "mpkSize": 1, "source": "registry",
            "services": { "": "b1" }, "installedBy": by, "installedAt": 1,
            "visibility": visibility, "orgs": orgs.iter().map(|o| p(o)).collect::<Vec<_>>(),
        }))
        .unwrap(),
    };
    assert_eq!(
        m.run_authorized(install("carol", &["bob"], "private")),
        Err(NOT_AN_ORG_MEMBER.into())
    );
    let _ok = m
        .run_authorized(install("alice", &["bob"], "private"))
        .unwrap();
    let seen = |m: &SystemMachine, who: &str| {
        m.state
            .sees_release(&m.state.packages["p"].versions["1.0.0"], who)
    };
    assert!(seen(&m, "alice") && seen(&m, "bob"));
    assert!(!seen(&m, "carol"), "sharing A with its installer is not B");
    assert!(!seen(&m, "dave") && !seen(&m, OPERATOR));

    // Carol reaches an instance of it: she reads the release it runs, never uses it.
    let app: crate::types::AppRecord = serde_json::from_value(json!({
        "appId": "x", "name": "x", "group": 2, "bundle": "b1", "owner": "bob",
        "org": p("bob"), "replicas": [1], "createdAt": 0, "grants": [{ "kind": "anyone" }],
        "release": { "package": "p", "version": "1.0.0" },
    }))
    .unwrap();
    let _prev = m.state.apps.insert("x".into(), app);
    let v = m.state.packages["p"].versions["1.0.0"].clone();
    assert!(m.state.may_reach(&m.state.apps["x"], Some("carol")));
    assert!(m.state.reads_release("p", &v, "carol") && !m.state.sees_release(&v, "carol"));
    assert!(!m.state.reads_release("p", &v, OPERATOR));

    // Installed again for A: A's members use it too, a guest does not.
    let _ok = m
        .run_authorized(install("alice", &["alice"], "private"))
        .unwrap();
    assert!(seen(&m, "carol") && !seen(&m, "dave"));
    // An installer who leaves loses it with the rest of the organization.
    for org in ["alice", "bob"] {
        let _gone = m
            .state
            .orgs
            .get_mut(&p(org))
            .unwrap()
            .members
            .remove("alice");
    }
    assert!(!seen(&m, "alice") && seen(&m, "carol") && seen(&m, "bob"));
    assert!(
        !m.state.sees_bundle("b1", "alice"),
        "its first installer, by hash"
    );

    // Served to anyone, it is public, and a private install never takes that back.
    let _ok = m.run_authorized(install("dave", &[], "public")).unwrap();
    let _ok = m
        .run_authorized(install("bob", &["bob"], "private"))
        .unwrap();
    let v = &m.state.packages["p"].versions["1.0.0"];
    assert_eq!(v.visibility, Some(Visibility::Public));
    assert!(seen(&m, "dave") && seen(&m, "alice"));
}

/// Reaching an instance of one package reads nothing of another package's release that
/// ships the same WASM at the same version: Bob runs public `notes@1.0.0`, never reads
/// Alice's private `secret@1.0.0`.
#[test]
fn reaching_an_instance_never_reads_another_packages_release_of_its_wasm() {
    let mut m = SystemMachine::default();
    let _ok = m.run_authorized(genesis(all(Authority::Owner))).unwrap();
    for who in ["alice", "bob"] {
        let _ok = m.run_authorized(account(who)).unwrap();
    }
    for (package, by, visibility) in [("secret", "alice", "private"), ("notes", "bob", "public")] {
        let _ok = m
            .run_authorized(SystemCmd::InstallPackage {
                package: package.into(),
                signer_id: "did:key:z".into(),
                release: serde_json::from_value(json!({
                    "version": "1.0.0", "mpk": package, "mpkSize": 1, "source": "registry",
                    "services": { "": "b1" }, "installedBy": by, "installedAt": 1,
                    "visibility": visibility, "orgs": [p(by)],
                }))
                .unwrap(),
            })
            .unwrap();
    }
    let app: crate::types::AppRecord = serde_json::from_value(json!({
        "appId": "x", "name": "x", "group": 2, "bundle": "b1", "owner": "bob",
        "org": p("bob"), "replicas": [1], "createdAt": 0,
        "release": { "package": "notes", "version": "1.0.0" },
    }))
    .unwrap();
    let _prev = m.state.apps.insert("x".into(), app);
    assert!(m.state.newest_read("notes", "bob", false).is_some());
    assert!(m.state.newest_read("secret", "bob", false).is_none());
}

/// A private release installed for no organization (its installer is in none) is its
/// installing accounts' alone; an install for an organization adds it, and from then on
/// the organization's members use it, an installer only while one.
#[test]
fn a_private_release_for_no_organization_is_its_installers_alone() {
    let mut m = SystemMachine::default();
    let _ok = m.run_authorized(genesis(all(Authority::Owner))).unwrap();
    for who in ["alice", "bob", "carol"] {
        let _ok = m.run_authorized(account(who)).unwrap();
    }
    let install = |by: &str, orgs: &[String]| SystemCmd::InstallPackage {
        package: "p".into(),
        signer_id: "did:key:z".into(),
        release: serde_json::from_value(json!({
            "version": "1.0.0", "mpk": "ab", "mpkSize": 1, "source": "upload",
            "services": { "": "b1" }, "installedBy": by, "installedAt": 1,
            "visibility": "private", "orgs": orgs,
        }))
        .unwrap(),
    };
    let seen = |m: &SystemMachine, who: &str| {
        m.state
            .sees_release(&m.state.packages["p"].versions["1.0.0"], who)
    };
    let _ok = m.run_authorized(install("alice", &[])).unwrap();
    assert!(seen(&m, "alice"));
    assert!(!seen(&m, "bob") && !seen(&m, "carol") && !seen(&m, OPERATOR));
    let _ok = m.run_authorized(install("bob", &[])).unwrap();
    assert!(seen(&m, "bob") && !seen(&m, "carol"));

    let _ok = m.run_authorized(install("carol", &[p("carol")])).unwrap();
    assert!(seen(&m, "carol"));
    assert!(
        !seen(&m, "alice") && !seen(&m, "bob"),
        "the organization's rule now"
    );
}

/// A package's first installer uninstalls (and manages the origins of) only the releases it
/// may use, from the logged version that brings private releases; an entry logged below it
/// replays as it applied.
#[test]
fn an_uninstall_takes_only_the_releases_its_actor_may_use() {
    use super::cluster_version::Feature;
    let mut m = SystemMachine::default();
    let _ok = m.run_authorized(genesis(all(Authority::Owner))).unwrap();
    for who in ["alice", "bob"] {
        let _ok = m.run_authorized(account(who)).unwrap();
    }
    let install =
        |by: &str, version: &str, visibility: &str, orgs: &[String]| SystemCmd::InstallPackage {
            package: "p".into(),
            signer_id: "did:key:z".into(),
            release: serde_json::from_value(json!({
                "version": version, "mpk": version, "mpkSize": 1, "source": "registry",
                "services": { "": format!("b-{version}") }, "installedBy": by,
                "installedAt": 1, "visibility": visibility, "orgs": orgs,
            }))
            .unwrap(),
        };
    let _ok = m
        .run_authorized(install("bob", "1.0.0", "public", &[]))
        .unwrap();
    let _ok = m
        .run_authorized(install("alice", "2.0.0", "private", &[p("alice")]))
        .unwrap();
    let uninstall = |version: Option<&str>| SystemCmd::UninstallPackage {
        package: "p".into(),
        version: version.map(str::to_owned),
        actor: "bob".into(),
    };

    let mut old = SystemMachine {
        state: m.state.clone(),
        ..Default::default()
    };
    let gone = old.run_authorized(uninstall(Some("2.0.0"))).unwrap();
    assert_eq!(gone["removed"], json!(["2.0.0"]), "replayed as it applied");

    m.state.cluster_version = Some(Feature::PrivateReleases.since().to_owned());
    assert_eq!(
        m.run_authorized(uninstall(Some("2.0.0"))),
        m.run_authorized(uninstall(Some("9.9.9")))
    );
    let gone = m.run_authorized(uninstall(None)).unwrap();
    assert_eq!(gone["removed"], json!(["1.0.0"]));
    assert!(m.state.packages["p"].versions.contains_key("2.0.0"));
    assert_eq!(
        m.run_authorized(uninstall(None)),
        Err("no such package".into())
    );
    // Nor does the first installer manage the origins of releases it may not use.
    assert!(!m.state.manages_package("p", "bob"));
    let mut before = m.state.clone();
    before.cluster_version = None;
    assert!(before.manages_package("p", "bob"), "as it applied before");
}

/// An app team is named, and a package named outright is found, only from the releases its
/// actor may use, from the logged version that brings private releases: a package with
/// none answers as a missing one, and a team (by a scope or a move) is named after the
/// newest release the actor may use. Entries logged below it apply as they did.
#[test]
fn an_app_team_reads_only_the_releases_its_actor_may_use() {
    use super::cluster_version::Feature;
    let mut m = SystemMachine::default();
    let _ok = m.run_authorized(genesis(all(Authority::Owner))).unwrap();
    let _ok = m.run_authorized(account("alice")).unwrap();
    for id in ["org", "dst"] {
        let mut o = org(&[("alice", "owner")], &[], &["2.0.0"]);
        id.clone_into(&mut o.org_id);
        let _prev = m.state.orgs.insert(id.into(), o);
    }
    let release = |version: &str, visibility: &str, name: &str| {
        json!({
            "version": version, "mpk": version, "mpkSize": 1, "source": "upload",
            "services": { "": format!("b-{version}") }, "installedBy": "bob",
            "installedAt": 1, "visibility": visibility, "orgs": ["elsewhere"],
            "metadata": { "name": name }
        })
    };
    for (package, versions) in [
        (
            "p",
            json!({ "1.0.0": release("1.0.0", "public", "Open"),
                    "2.0.0": release("2.0.0", "private", "Secret") }),
        ),
        (
            "q",
            json!({ "1.0.0": release("1.0.0", "private", "Hidden") }),
        ),
        ("r", json!({ "1.0.0": release("1.0.0", "public", "Other") })),
    ] {
        let _prev = m.state.packages.insert(
            package.into(),
            serde_json::from_value(json!({
                "name": package, "signerId": "did:key:z", "owner": "bob", "versions": versions
            }))
            .unwrap(),
        );
    }
    let _prev = m.state.bundles.insert(
        "b-1.0.0".into(),
        serde_json::from_value(json!({
            "hash": "b-1.0.0", "size": 1, "uploadedBy": "bob",
            "package": { "package": "p", "version": "1.0.0" }
        }))
        .unwrap(),
    );
    let _prev = m.state.apps.insert(
        "a".into(),
        serde_json::from_value(json!({
            "appId": "a", "name": "a", "group": 5, "bundle": "b-1.0.0", "owner": "alice",
            "org": "org", "grants": [{ "kind": "org" }], "replicas": [1], "createdAt": 0,
            "release": { "package": "p", "version": "1.0.0" }
        }))
        .unwrap(),
    );
    let scope_in = |org: &str, package: &str| SystemCmd::ScopeApps {
        org: org.into(),
        apps: Some(vec![super::ScopeApp {
            package: package.into(),
            name: None,
            instances: Some(Vec::new()),
            join: None,
            keep: None,
            child_teams: None,
        }]),
        shared: None,
        nonce: package.into(),
        actor: "alice".into(),
        consent: Value::Null,
        member_child_teams: true,
        all_apps_rows: true,
        by_release: true,
    };
    let scope = |package: &str| scope_in("org", package);
    // A target with an app team gives a moved app one of its own.
    let _ok = m.run_unproven(scope_in("dst", "r")).unwrap();
    let team_name = |m: &SystemMachine, org: &str, package: &str| {
        m.state.orgs[org].app_team(package).map(|t| t.name.clone())
    };

    let mut old = SystemMachine {
        state: m.state.clone(),
        ..Default::default()
    };
    let _ok = old.run_unproven(scope("p")).unwrap();
    let _ok = old.run_unproven(scope("q")).unwrap();
    assert_eq!(
        team_name(&old, "org", "p").as_deref(),
        Some("Secret"),
        "as it applied"
    );
    let _ok = old
        .run_unproven(move_app("a", "dst", "alice", false, Value::Null))
        .unwrap();
    assert_eq!(
        team_name(&old, "dst", "p").as_deref(),
        Some("Secret"),
        "as it applied"
    );

    m.state.cluster_version = Some(Feature::PrivateReleases.since().to_owned());
    assert_eq!(m.run_unproven(scope("q")), m.run_unproven(scope("missing")));
    assert_eq!(team_name(&m, "org", "q"), None);
    let _ok = m.run_unproven(scope("p")).unwrap();
    assert_eq!(team_name(&m, "org", "p").as_deref(), Some("Open"));
    let _ok = m
        .run_unproven(move_app("a", "dst", "alice", false, Value::Null))
        .unwrap();
    assert_eq!(team_name(&m, "dst", "p").as_deref(), Some("Open"));
}

/// A release of another signer's package pins a private module only while it is private to
/// the module's own organizations, judged at apply on the release as the install leaves it
/// (an install again for another organization included), from the logged version that
/// brings private releases; an entry logged below it applies as it did.
#[test]
fn a_private_module_is_pinned_only_into_a_release_private_to_its_organizations() {
    use super::cluster_version::Feature;
    let mut m = SystemMachine::default();
    let _ok = m.run_authorized(genesis(all(Authority::Owner))).unwrap();
    for who in ["vic", "mia"] {
        let _ok = m.run_authorized(account(who)).unwrap();
    }
    for (id, members) in [
        ("v", &[("vic", "owner"), ("mia", "member")][..]),
        ("a", &[("mia", "owner")][..]),
    ] {
        let mut o = org(members, &[], &[]);
        id.clone_into(&mut o.org_id);
        let _prev = m.state.orgs.insert(id.into(), o);
    }
    let install = |package: &str, signer: &str, by: &str, org: &str, extra: Value| {
        let mut release = json!({
            "version": "1.0.0", "mpk": package, "mpkSize": 1, "source": "upload",
            "services": {}, "installedBy": by, "installedAt": 1,
            "visibility": "private", "orgs": [org],
        });
        release
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        SystemCmd::InstallPackage {
            package: package.into(),
            signer_id: signer.into(),
            release: serde_json::from_value(release).unwrap(),
        }
    };
    let module = install(
        "m",
        "did:key:victim",
        "vic",
        "v",
        json!({ "compute": { "": "c" } }),
    );
    let _ok = m.run_authorized(module).unwrap();
    let q = |org: &str| {
        install(
            "q",
            "did:key:attacker",
            "mia",
            org,
            json!({
                "compute": { "": "cq" },
                "pipelines": { "p1": { "hash": "h", "worstCaseFuel": "1", "modules": {
                    "m@1.0.0": { "package": "m", "version": "1.0.0", "compute": "c" }
                } } }
            }),
        )
    };

    let mut old = SystemMachine {
        state: m.state.clone(),
        ..Default::default()
    };
    let _ok = old.run_authorized(q("a")).expect("replayed as it applied");

    m.state.cluster_version = Some(Feature::PrivateReleases.since().to_owned());
    assert_eq!(
        m.run_authorized(q("a")),
        Err("a module the pipelines use is no longer installed".into())
    );
    assert!(!m.state.packages.contains_key("q"));
    let _ok = m.run_authorized(q("v")).unwrap();
    assert_eq!(
        m.run_authorized(q("a")),
        Err("a module the pipelines use is no longer installed".into())
    );
    assert_eq!(
        m.state.packages["q"].versions["1.0.0"].orgs,
        BTreeSet::from(["v".to_owned()])
    );
}

/// An outbound approval approves the prefixes of the newest release its approver reads, from
/// the logged version that brings private releases: another organization's private newer
/// release neither blocks it nor lends it its prefixes; one with none it reads is a missing
/// package. An entry logged below that version approves the newest release's, as it did.
#[test]
fn an_outbound_approval_takes_the_newest_release_its_approver_reads() {
    use super::cluster_version::Feature;
    let mut m = SystemMachine::default();
    let _ok = m.run_authorized(genesis(all(Authority::Owner))).unwrap();
    let _ok = m.run_authorized(account("alice")).unwrap();
    let _prev = m
        .state
        .orgs
        .insert("org".into(), org(&[("alice", "owner")], &[], &[]));
    let release = |version: &str, visibility: &str, prefix: &str| {
        json!({
            "version": version, "mpk": version, "mpkSize": 1, "source": "upload",
            "services": { "": format!("b-{version}") }, "installedBy": "bob",
            "installedAt": 1, "visibility": visibility, "orgs": ["elsewhere"],
            "outbound": [prefix]
        })
    };
    let _prev = m.state.packages.insert(
        "p".into(),
        serde_json::from_value(json!({
            "name": "p", "signerId": "did:key:z", "owner": "bob", "versions": {
                "1.0.0": release("1.0.0", "public", "https://open.example/"),
                "2.0.0": release("2.0.0", "private", "https://secret.example/"),
            }
        }))
        .unwrap(),
    );
    let outbound = SystemCmd::SetOutboundApproval {
        scope: ReleaseScope::Org { org: "org".into() },
        package: "p".into(),
        selection: calimero_primitives::outbound::OutboundSelection::Manifest,
        replace: false,
        actor: "alice".into(),
        consent: Value::Null,
    };
    let mut old = SystemMachine {
        state: m.state.clone(),
        ..Default::default()
    };
    let got = old.run_unproven(outbound.clone()).unwrap();
    assert_eq!(
        got["approval"]["prefixes"],
        json!(["https://secret.example/"]),
        "as it applied"
    );

    m.state.cluster_version = Some(Feature::PrivateReleases.since().to_owned());
    let got = m.run_unproven(outbound.clone()).unwrap();
    assert_eq!(
        got["approval"]["prefixes"],
        json!(["https://open.example/"])
    );
    let v1 = m
        .state
        .packages
        .get_mut("p")
        .unwrap()
        .versions
        .get_mut("1.0.0")
        .unwrap();
    v1.visibility = Some(super::Visibility::Private);
    assert_eq!(m.run_unproven(outbound), Err("no such package".into()));
}

/// A release recorded before `visibility` keeps the rule from before: whoever shares an
/// organization with an installer, or reaches an app of it, uses it. Its entry writes back
/// byte for byte; a private install of it again leaves that rule, a public one replaces it.
#[test]
fn a_release_recorded_before_visibility_keeps_the_rule_from_before() {
    use super::{PackageVersion, Visibility};
    let old = json!({
        "version": "1.0.0", "mpk": "ab", "mpkSize": 1, "source": "registry",
        "services": { "": "b1" }, "metadata": null, "outbound": [],
        "installedBy": "alice", "installedAt": 1,
    });
    let release: PackageVersion = serde_json::from_value(old.clone()).unwrap();
    assert!(release.visibility.is_none() && release.orgs.is_empty());
    assert_eq!(serde_json::to_value(&release).unwrap(), old);
    let entry = SystemCmd::InstallPackage {
        package: "p".into(),
        signer_id: "did:key:z".into(),
        release: release.clone(),
    };
    assert!(entry.features().is_empty());

    let mut m = SystemMachine::default();
    let _ok = m.run_authorized(genesis(all(Authority::Owner))).unwrap();
    for who in ["alice", "bob", "carol"] {
        let _ok = m.run_authorized(account(who)).unwrap();
    }
    let _prev = m
        .state
        .orgs
        .get_mut(&p("alice"))
        .unwrap()
        .members
        .insert("bob".into(), OrgRole::Member);
    let _ok = m.run_authorized(raw_bundle("carol", "b1", 1)).unwrap();
    let _ok = m.run_authorized(entry).unwrap();
    assert_eq!(
        serde_json::to_value(&m.state.packages["p"].versions["1.0.0"]).unwrap(),
        old
    );
    assert!(
        m.state.sees_bundle("b1", "carol"),
        "its raw uploader, as before"
    );
    let seen = |m: &SystemMachine, who: &str| {
        m.state
            .sees_release(&m.state.packages["p"].versions["1.0.0"], who)
    };
    assert!(seen(&m, "alice") && seen(&m, "bob") && !seen(&m, "carol"));
    assert!(m
        .state
        .sees_release_from(&m.state.packages["p"].versions["1.0.0"], "carol", true));

    let again = |visibility: Visibility| SystemCmd::InstallPackage {
        package: "p".into(),
        signer_id: "did:key:z".into(),
        release: PackageVersion {
            installed_by: "bob".into(),
            visibility: Some(visibility),
            orgs: [p("bob")].into(),
            ..release.clone()
        },
    };
    let _ok = m.run_authorized(again(Visibility::Private)).unwrap();
    let v = &m.state.packages["p"].versions["1.0.0"];
    assert!(v.visibility.is_none() && seen(&m, "bob"));
    let _ok = m.run_authorized(again(Visibility::Public)).unwrap();
    assert!(seen(&m, "carol"));
    assert!(!m.state.sees_release_from(
        &PackageVersion {
            visibility: Some(Visibility::Private),
            ..release
        },
        "carol",
        true
    ));
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
            components: Default::default(),
            pipelines: Default::default(),
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
            secrets: BTreeMap::new(),
            uses: BTreeMap::new(),
            installed_by: "alice".into(),
            installed_at: 0,
            installers: BTreeSet::new(),
            upgrades: UpgradeAuthority::Owner,
            min_runtime_version: None,
            instance_org: None,
            guests: None,
            visibility: None,
            orgs: BTreeSet::new(),
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
    assert!(e.message().contains("compute component"), "{e}");
    let with_compute = |id: &str, compute: &str| {
        SystemCmd::CreateApp {
        app: Box::new(
            serde_json::from_value(json!({
                "appId": id, "name": id, "group": 0, "bundle": "m1", "owner": "alice", "org": p("alice"),
                "replicas": [1], "createdAt": 10, "computeBundle": compute,
            }))
            .unwrap(),
        ),
        unit_offer: None,
        offers_only: false,
    }
    };
    let e = m.run_unproven(with_compute("bad", "j2")).unwrap_err();
    assert!(e.message().contains("not shipped"), "{e}");
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
        in_deployment: false,
        owner_approves: false,
        class: None,
        auto: false,
        members_stay: false,
        restore_point: None,
    };
    let e = m.run_unproven(upgrade("j1", None)).unwrap_err();
    assert!(e.message().contains("compute component"), "{e}");
    let e = m.run_unproven(upgrade("m2", Some("j1"))).unwrap_err();
    assert!(e.message().contains("not shipped"), "{e}");
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
            components: Default::default(),
            pipelines: Default::default(),
            version: version.into(),
            mpk: format!("mpk{version}"),
            mpk_size: 40,
            source: BundleSource::Upload,
            services: BTreeMap::from([(String::new(), main.to_owned())]),
            compute: BTreeMap::from([(String::new(), job.to_owned())]),
            part_sizes: BTreeMap::from([(main.to_owned(), 30), (job.to_owned(), 7)]),
            metadata: Value::Null,
            outbound: Vec::new(),
            secrets: BTreeMap::new(),
            uses: BTreeMap::new(),
            installed_by: "alice".into(),
            installed_at: 0,
            installers: BTreeSet::new(),
            upgrades: UpgradeAuthority::Owner,
            min_runtime_version: None,
            instance_org: None,
            guests: None,
            visibility: None,
            orgs: BTreeSet::new(),
        },
    };
    let _ok = m.run_authorized(release("1.0.0", "m1", "j1")).unwrap();
    assert_eq!(m.state.bundles["m1"].size, 30);
    assert_eq!(m.state.bundles["j1"].size, 7);
    let e = m.run_authorized(release("1.1.0", "m2", "j2")).unwrap_err();
    assert!(e.message().contains("quota exceeded"), "{e}");
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
            components: Default::default(),
            pipelines: Default::default(),
            version: version.into(),
            mpk: format!("mpk{version}"),
            mpk_size: 1,
            source: BundleSource::Upload,
            services: BTreeMap::from([(String::new(), main.to_owned())]),
            compute: BTreeMap::from([(String::new(), job.to_owned())]),
            part_sizes: BTreeMap::new(),
            metadata: Value::Null,
            outbound: Vec::new(),
            secrets: BTreeMap::new(),
            uses: BTreeMap::new(),
            installed_by: "alice".into(),
            installed_at: 0,
            installers: BTreeSet::new(),
            upgrades: UpgradeAuthority::Owner,
            min_runtime_version: None,
            instance_org: None,
            guests: None,
            visibility: None,
            orgs: BTreeSet::new(),
        },
    };
    let _ok = m.run_authorized(install("1.0.0", "m1", "j1")).unwrap();
    let _ok = m.run_authorized(install("1.1.0", "m1", "j2")).unwrap();
    // The app runs the main component both releases share, and the first one's compute.
    let app = SystemCmd::CreateApp {
        app: Box::new(
            serde_json::from_value(json!({
                "appId": "app", "name": "app", "group": 0, "bundle": "m1", "owner": "alice", "org": p("alice"),
                "replicas": [1], "createdAt": 10, "computeBundle": "j1",
            }))
            .unwrap(),
        ),
        unit_offer: None,
        offers_only: false,
    };
    let _ok = m.run_unproven(app).unwrap();
    let uninstall = |version: &str| SystemCmd::UninstallPackage {
        package: "p".into(),
        version: Some(version.into()),
        actor: "alice".into(),
    };
    let e = m.run_unproven(uninstall("1.0.0")).unwrap_err();
    assert!(e.message().contains("still runs"), "{e}");
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
        "passkeys": [credential],
    }))
    .unwrap();
    let args = super::account_args(&account, None);
    SystemCmd::CreateAccount {
        at: 0,
        consent: consent(pk, &format!("account:{id}"), "__create_account", &args),
        account,
        invite: None,
        refresh: None,
        personal_org: Some(super::PersonalOrgSeed {
            org_id: p(id),
            release: Some("1.0.0".into()),
        }),
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
        scope: ReleaseScope::Org { org: p(actor) },
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
            let mut c = consent(&mut alice, "org:p-alice", "__approve_release", &args);
            c["assertion"].take()
        }),
        (
            "bob's passkey",
            consent(&mut bob, "org:p-alice", "__approve_release", &args),
        ),
        (
            "bob's subject",
            consent(&mut alice, "org:p-bob", "__approve_release", &args),
        ),
        (
            "a withdrawal",
            consent(&mut alice, "org:p-alice", "__withdraw_release", &args),
        ),
        (
            "another release",
            consent(
                &mut alice,
                "org:p-alice",
                "__approve_release",
                &json!({ "version": "3.0.0" }),
            ),
        ),
    ] {
        assert!(m.run(approve_with("alice", "2.0.0", c)).is_err(), "{why}");
    }
    assert_eq!(m.state.orgs[&p("alice")].approved_releases.len(), 1);

    let real = consent(&mut alice, "org:p-alice", "__approve_release", &args);
    let _ok = m.run(approve_with("alice", "2.0.0", real.clone())).unwrap();
    assert!(m.state.orgs[&p("alice")]
        .approved_releases
        .contains("2.0.0"));
    // Withdrawn, then the old consent logged again: refused.
    let w = consent(&mut alice, "org:p-alice", "__withdraw_release", &args);
    let _ok = m
        .run(SystemCmd::SetReleaseApproval {
            scope: ReleaseScope::Org { org: p("alice") },
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
    assert!(!m.state.orgs[&p("alice")]
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
        in_deployment: false,
        owner_approves: false,
        class: None,
        auto: false,
        members_stay: false,
        restore_point: None,
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

/// `proven_cluster` plus package `p` with releases 1.0.0, 2.0.0 and
/// 3.0.0 (WASM `b-<version>`), and three apps of `org`: `p1` on 1.0.0,
/// `p3` on 3.0.0 and `q` on a raw bundle.
fn org_with_package_apps() -> (SystemMachine, Soft) {
    let (mut m, alice, _bob, _carol) = proven_cluster();
    let versions: serde_json::Map<String, serde_json::Value> = ["1.0.0", "2.0.0", "3.0.0"]
        .iter()
        .map(|v| {
            let release = json!({
                "version": v, "mpk": v, "mpkSize": 1, "source": "upload",
                "services": { "": format!("b-{v}") }, "installedBy": "alice", "installedAt": 0
            });
            ((*v).to_owned(), release)
        })
        .collect();
    let _prev = m.state.packages.insert(
        "p".into(),
        serde_json::from_value(json!({
            "name": "p", "signerId": "did:key:z", "owner": "alice", "versions": versions
        }))
        .unwrap(),
    );
    for v in ["1.0.0", "2.0.0", "3.0.0"] {
        let hash = format!("b-{v}");
        let _prev = m.state.bundles.insert(
            hash.clone(),
            serde_json::from_value(json!({
                "hash": hash, "size": 1, "uploadedBy": "alice",
                "package": { "package": "p", "version": v }
            }))
            .unwrap(),
        );
    }
    for (id, bundle, release) in [
        ("p1", "b-1.0.0", Some("1.0.0")),
        ("p3", "b-3.0.0", Some("3.0.0")),
        ("q", "b1", None),
    ] {
        let _prev = m.state.apps.insert(
            id.into(),
            serde_json::from_value(json!({
                "appId": id, "name": id, "group": 7, "bundle": bundle, "owner": "alice",
                "org": "org", "grants": [{ "kind": "org" }], "replicas": [1, 2, 3],
                "createdAt": 0,
                "release": release.map(|v| json!({ "package": "p", "version": v })),
            }))
            .unwrap(),
        );
    }
    (m, alice)
}

/// From the `RequireCurrentRules` mark on, an organization's upgrade run
/// covers what the state says its consent covers, whatever apps or
/// progress the entry lists, and moves each app once, along its package
/// and never to an older release. Before the mark a run applies as it
/// was logged.
#[test]
fn an_upgrade_run_covers_what_its_consent_covers_and_moves_each_app_once() {
    use super::{NOT_IN_RUN, RUN_MOVED_APP, RUN_OLDER_RELEASE};
    let (mut m, mut alice) = org_with_package_apps();
    let start = |alice: &mut Soft, run_id: &str| {
        let c = consent(
            alice,
            "org:org",
            "__upgrade_org",
            &json!({
                "package": "p", "version": "2.0.0", "team": null,
                "cascade": false, "migrate": null,
            }),
        );
        let run: crate::types::UpgradeRun = serde_json::from_value(json!({
            "runId": run_id, "package": "p", "version": "2.0.0", "by": "alice",
            "startedAt": 0, "apps": ["p1", "p3", "q", "app"], "done": ["p1"],
            "failed": { "p3": "made up" }, "finished": true,
        }))
        .unwrap();
        SystemCmd::StartUpgradeRun {
            org: "org".into(),
            run: Box::new(run),
            consent: c,
            by_release: false,
            owner_approves: false,
        }
    };
    let upgrade = |app: &str, bundle: &str, version: &str| SystemCmd::UpgradeApp {
        compute_bundle: None,
        app_id: app.into(),
        bundle: bundle.into(),
        by_operator: false,
        actor: "alice".into(),
        at: 0,
        run: Some("r1".into()),
        consent: serde_json::Value::Null,
        release: Some(super::PackageRef {
            package: "p".into(),
            version: version.into(),
            service: None,
            compute: false,
        }),
        owner_in_org: true,
        owner_in_team: true,
        in_deployment: false,
        owner_approves: false,
        class: None,
        auto: false,
        members_stay: false,
        restore_point: None,
    };

    // Logged before the mark: kept as the entry says, as it was.
    m.index = 5;
    let _ok = m.run(start(&mut alice, "r0")).unwrap();
    assert_eq!(m.state.orgs["org"].upgrade_runs["r0"].apps.len(), 4);

    m.index = 10;
    let _ok = m.run(SystemCmd::RequireCurrentRules).unwrap();
    m.index = 11;
    let _ok = m.run(start(&mut alice, "r1")).unwrap();
    let r = &m.state.orgs["org"].upgrade_runs["r1"];
    assert_eq!(
        r.apps,
        ["p1", "p3"],
        "the org's apps of `p`, from the state"
    );
    assert!(r.done.is_empty() && r.failed.is_empty() && !r.finished);

    assert_eq!(
        m.run(upgrade("q", "b-2.0.0", "2.0.0")).unwrap_err(),
        NOT_IN_RUN
    );
    assert_eq!(
        m.run(upgrade("p3", "b-2.0.0", "2.0.0")).unwrap_err(),
        RUN_OLDER_RELEASE
    );
    let _ok = m.run(upgrade("p1", "b-2.0.0", "2.0.0")).unwrap();
    assert_eq!(m.state.apps["p1"].bundle, "b-2.0.0");
    assert_eq!(
        m.run(upgrade("p1", "b-2.0.0", "2.0.0")).unwrap_err(),
        RUN_MOVED_APP
    );
    // A run id is one run in the cluster.
    assert_eq!(
        m.run(start(&mut alice, "r1")).unwrap_err(),
        "that run exists"
    );
}

/// From the logged version that brings private releases, an app's move in a run applies only
/// to a release its owner may use, whoever started the run; an entry logged below it applies
/// as it did.
#[test]
fn a_run_moves_an_app_only_to_a_release_its_owner_may_use() {
    use super::apps::RELEASE_NOT_USABLE;
    use super::cluster_version::Feature;
    let (mut m, mut alice) = org_with_package_apps();
    m.index = 10;
    let _ok = m.run(SystemCmd::RequireCurrentRules).unwrap();
    let c = consent(
        &mut alice,
        "org:org",
        "__upgrade_org",
        &json!({
            "package": "p", "version": "2.0.0", "team": null,
            "cascade": false, "migrate": null,
        }),
    );
    let run: crate::types::UpgradeRun = serde_json::from_value(json!({
        "runId": "r1", "package": "p", "version": "2.0.0", "by": "alice",
        "startedAt": 0, "apps": [], "done": [], "failed": {}, "finished": false,
    }))
    .unwrap();
    m.index = 11;
    let _ok = m
        .run(SystemCmd::StartUpgradeRun {
            org: "org".into(),
            run: Box::new(run),
            consent: c,
            by_release: false,
            owner_approves: false,
        })
        .unwrap();
    let release = m
        .state
        .packages
        .get_mut("p")
        .and_then(|p| p.versions.get_mut("2.0.0"))
        .unwrap();
    release.visibility = Some(super::Visibility::Private);
    release.orgs = ["elsewhere".to_owned()].into();
    let upgrade = || {
        let mut cmd = upgrade_to("p1", "alice", "b-2.0.0", Some(("p", "2.0.0")));
        if let SystemCmd::UpgradeApp { run, .. } = &mut cmd {
            *run = Some("r1".into());
        }
        cmd
    };

    m.state.cluster_version = Some(Feature::PrivateReleases.since().to_owned());
    m.index = 12;
    assert_eq!(m.run(upgrade()).unwrap_err(), RELEASE_NOT_USABLE);
    assert_eq!(m.state.apps["p1"].bundle, "b-1.0.0");

    m.state.cluster_version = None;
    let _ok = m.run(upgrade()).unwrap();
    assert_eq!(m.state.apps["p1"].bundle, "b-2.0.0");
}

/// A publisher's run covers the instances that run a release of its package
/// (`SystemState::app_release`), as an organization's run and each app's
/// `UpgradeApp { run }` judge it: not the instances whose WASM another
/// package's release shipped first. An entry logged before `by_release`
/// (absent) replays judging each app by the package its bundle was first
/// recorded from, as it did then.
#[test]
fn a_publishers_run_covers_the_instances_that_run_its_package() {
    let (mut m, mut alice) = org_with_package_apps();
    // `x` 1.0.0 ships p's 2.0.0 component, and `bx` was recorded from `x`.
    let _prev = m.state.packages.insert(
        "x".into(),
        serde_json::from_value(json!({
            "name": "x", "signerId": "did:key:zx", "owner": "alice", "versions": { "1.0.0": {
                "version": "1.0.0", "mpk": "x1", "mpkSize": 1, "source": "upload",
                "services": { "": "b-2.0.0" }, "installedBy": "alice", "installedAt": 0
            } }
        }))
        .unwrap(),
    );
    let _prev = m.state.bundles.insert(
        "bx".into(),
        serde_json::from_value(json!({
            "hash": "bx", "size": 1, "uploadedBy": "alice",
            "package": { "package": "x", "version": "1.0.0" }
        }))
        .unwrap(),
    );
    for (id, bundle, package, version) in [
        // Runs `x`, on a component first recorded from `p`.
        ("xa", "b-2.0.0", "x", "1.0.0"),
        // Runs `p`, on a component first recorded from `x`.
        ("pb", "bx", "p", "1.0.0"),
    ] {
        let _prev = m.state.apps.insert(
            id.into(),
            serde_json::from_value(json!({
                "appId": id, "name": id, "group": 7, "bundle": bundle, "owner": "alice",
                "org": "org", "grants": [{ "kind": "org" }], "replicas": [1, 2, 3],
                "createdAt": 0, "release": { "package": package, "version": version },
            }))
            .unwrap(),
        );
    }
    m.index = 10;
    let _ok = m.run(SystemCmd::RequireCurrentRules).unwrap();
    let mut start = |run_id: &str, by_release: bool| {
        let c = consent(
            &mut alice,
            "org:org",
            "__upgrade_published",
            &json!({ "package": "p", "version": "3.0.0", "migrate": null }),
        );
        let run: crate::types::UpgradeRun = serde_json::from_value(json!({
            "runId": run_id, "package": "p", "version": "3.0.0", "by": "alice",
            "startedAt": 0, "apps": [], "done": [], "failed": {}, "finished": false,
        }))
        .unwrap();
        m.index += 1;
        let _ok = m
            .run(SystemCmd::StartUpgradeRun {
                org: "org".into(),
                run: Box::new(run),
                consent: c,
                by_release,
                owner_approves: false,
            })
            .unwrap();
        let mut apps = m.state.orgs["org"].upgrade_runs[run_id].apps.clone();
        apps.sort();
        apps
    };
    assert_eq!(start("now", true), ["p1", "p3", "pb"]);
    assert_eq!(
        start("before", false),
        ["p1", "p3", "xa"],
        "an entry from before replays by the bundle's first release"
    );
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
            "owner": "alice", "org": p("alice"), "replicas": [1, 2, 3], "createdAt": 0
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
    let args = super::api_key_args("h1", &record, &[]);
    let mut wider = args.clone();
    wider["readOnly"] = json!(false);
    let create = |consent| SystemCmd::CreateApiKey {
        hash: "h1".into(),
        record: record.clone(),
        consent,
        owner_in_org: true,
        owner_in_team: true,
        owner_approves: false,
        then: Vec::new(),
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
            owner_approves: false,
            then: Vec::new(),
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

    // A key made with follow-up calls: its consent states them, and the entry
    // must carry the same ones.
    let then = vec![crate::types::SignedFollowUp {
        app: "app".into(),
        method: "setAccess".into(),
        args_hash: "a1".into(),
    }];
    let record = crate::types::ApiKeyRecord {
        key_id: "k2".into(),
        ..record
    };
    let args = super::api_key_args("h2", &record, &then);
    let with = |then: Vec<crate::types::SignedFollowUp>| {
        let record = record.clone();
        move |consent| SystemCmd::CreateApiKey {
            hash: "h2".into(),
            record: record.clone(),
            consent,
            owner_in_org: true,
            owner_in_team: true,
            owner_approves: false,
            then: then.clone(),
        }
    };
    let c = consent(&mut alice, "account:alice", "__create_api_key", &args);
    assert!(m.run(with(Vec::new())(c)).is_err(), "follow-ups dropped");
    let mut other = args.clone();
    other["then"][0]["argsHash"] = json!("a2");
    needs_consent(
        &mut m,
        &mut alice,
        ("account:alice", "__create_api_key"),
        &args,
        &other,
        with(then),
    );
    assert!(m.state.api_keys.contains_key("h2"));
}

/// A key's consent, and so the system log every node holds, states each
/// follow-up's arguments by a salted hash alone, never the app's data, and
/// that hash tells apart any two arguments that differ.
#[test]
fn a_keys_statement_names_follow_up_arguments_by_a_salted_hash_only() {
    let record: crate::types::ApiKeyRecord = serde_json::from_value(json!({
        "keyId": "k1", "account": "alice", "label": "ci", "expiresAt": 99,
    }))
    .unwrap();
    let call = |args: serde_json::Value| crate::types::KeyFollowUp {
        app: "app".into(),
        method: "setAccess".into(),
        args,
    };
    let hash = |args: serde_json::Value, salt: &[u8]| call(args).signed(salt).args_hash;
    let secret = json!({ "role": "writer", "bucket": "payroll-secret", "at": { "b": 1, "a": 2 } });
    let signed = call(secret.clone()).signed(b"salt-1");
    let stated = super::api_key_args("h1", &record, std::slice::from_ref(&signed)).to_string();
    assert!(!stated.contains("payroll-secret"), "{stated}");
    assert_eq!(
        stated,
        json!({ "hash": "h1", "keyId": "k1", "label": "ci", "apps": null, "methods": null,
            "readOnly": false, "allowConfirm": false, "expiresAt": 99,
            "then": [{ "app": "app", "method": "setAccess", "argsHash": signed.args_hash }] })
        .to_string()
    );
    // Key order, at any depth, does not matter.
    let reordered =
        json!({ "at": { "a": 2, "b": 1 }, "bucket": "payroll-secret", "role": "writer" });
    assert_eq!(hash(reordered, b"salt-1"), signed.args_hash);
    // Any difference does, past what a double holds and between 1 and 1.0.
    assert_ne!(
        hash(json!({ "role": "owner" }), b"salt-1"),
        hash(json!({ "role": "writer" }), b"salt-1")
    );
    let big = |n: &str| {
        serde_json::from_str::<serde_json::Value>(&format!(r#"{{"amount":{n}}}"#)).unwrap()
    };
    assert_ne!(
        hash(big("9007199254740993"), b"s"),
        hash(big("9007199254740992"), b"s")
    );
    assert_ne!(hash(big("1"), b"s"), hash(big("1.0"), b"s"));
    // The same arguments of another key, under its own salt, state another hash.
    assert_ne!(hash(secret, b"salt-2"), signed.args_hash);
}

/// A key without follow-ups states the bytes it did before they existed, so
/// its consent verifies on every release.
#[test]
fn a_plain_keys_statement_is_unchanged() {
    let record: crate::types::ApiKeyRecord = serde_json::from_value(json!({
        "keyId": "k1", "account": "alice", "label": "ci", "apps": ["app"],
        "methods": null, "readOnly": true, "allowConfirm": false, "expiresAt": 99,
    }))
    .unwrap();
    assert_eq!(
        super::api_key_args("h1", &record, &[]).to_string(),
        r#"{"hash":"h1","keyId":"k1","label":"ci","apps":["app"],"methods":null,"readOnly":true,"allowConfirm":false,"expiresAt":99}"#
    );
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

/// Review finding B1: every sign-in's proof stayed in `used_proofs` for good, so
/// the system state (one snapshot row) only grew. From the `ExpireProofs` mark
/// on, a sign-in's proof is forgotten once the session it opened expires (at an
/// entry's own time, never a clock), and a sign-in whose session expired by
/// then is refused instead: the forgotten proof cannot be logged again. Proofs
/// from before the mark are kept, as the releases that logged them kept them.
#[test]
fn sign_in_proofs_are_forgotten_once_their_session_expires() {
    let (mut m, mut alice, _bob, _carol) = proven_cluster();
    let mut sign_in = |m: &mut SystemMachine, session: &str, expires_at: u64| {
        let (_token, hash, mut record) = refresh_token("alice", &alice.credential_id(), session);
        record.expires_at = expires_at;
        let c = consent(
            &mut alice,
            "session",
            "__sign_in",
            &super::sign_in_args(&hash, &record),
        );
        let cmd = SystemCmd::CreateRefresh {
            hash,
            record,
            consent: c,
        };
        let used = super::authorize(&m.state, &cmd).unwrap().unwrap();
        let _ok = m.run(cmd.clone()).unwrap();
        (cmd, used)
    };
    let revoke = |m: &mut SystemMachine, session: &str, now: u64| {
        let _ok = m
            .run(SystemCmd::RevokeSession {
                session: session.into(),
                actor: "alice".into(),
                now,
            })
            .unwrap();
    };

    let (_, before_mark) = sign_in(&mut m, "s0", 10);
    let _ok = m.run(SystemCmd::ExpireProofs).unwrap();
    assert!(m.run(SystemCmd::ExpireProofs).is_err(), "written once");
    let (expiring, used) = sign_in(&mut m, "s1", 20);
    let (_, later) = sign_in(&mut m, "s2", 40);
    assert_eq!(m.state.proof_expiry[&20], [used.clone()].into());
    assert_eq!(m.state.proof_expiry[&40], [later.clone()].into());

    // Before its session expires, the proof is kept and a replay is refused by it.
    revoke(&mut m, "s1", 15);
    assert!(m.state.used_proofs.contains(&used));
    assert_eq!(
        m.run(expiring.clone()).unwrap_err(),
        "that consent was used already"
    );

    // Once it expired (an entry's `now`), it is forgotten; the one from before the
    // mark stays, and so does the one whose session has not expired.
    revoke(&mut m, "s1", 30);
    assert!(!m.state.used_proofs.contains(&used));
    assert!(m.state.used_proofs.contains(&before_mark));
    assert!(m.state.used_proofs.contains(&later));
    assert!(!m.state.proof_expiry.contains_key(&20));
    assert_eq!(m.state.proofs_forgotten_through, 20);

    // Logged again, the forgotten proof is refused by its signed expiry.
    let state = serde_json::to_value(&m.state).unwrap();
    assert_eq!(
        m.run(expiring).unwrap_err(),
        "that sign-in's session has expired: sign in again"
    );
    assert_eq!(serde_json::to_value(&m.state).unwrap(), state);
    // A sign-in whose session is still to come applies as before.
    let (_, fresh) = sign_in(&mut m, "s3", 50);
    assert!(m.state.used_proofs.contains(&fresh));
}

/// State from before the mark reads and writes as it did: none of the new fields
/// is written until `ExpireProofs` applies.
#[test]
fn proofs_from_before_the_expiry_mark_read_and_write_as_they_did() {
    let (m, _alice, _bob, _carol) = proven_cluster();
    let v = serde_json::to_value(&m.state).unwrap();
    for field in ["expiresProofs", "proofExpiry", "proofsForgottenThrough"] {
        assert!(v.get(field).is_none(), "{field}");
    }
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
            personal_org: None,
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
        consent(&mut alice, "org:p-bob", "__approve_release", &args),
        50,
    );
    assert!(m.run(approve_with("alice", "2.0.0", wrong)).is_err());
    let real = timed(
        consent(&mut alice, "org:p-alice", "__approve_release", &args),
        50,
    );
    let _ok = m.run(approve_with("alice", "2.0.0", real)).unwrap();
    let approved = ActivityEntry {
        at: 50,
        passkey: cred.clone(),
        what: Activity::Approval {
            method: "__approve_release".into(),
            subject: "org:p-alice".into(),
        },
    };
    assert_eq!(
        m.state.activity.get("alice").cloned().unwrap_or_default(),
        [signed_in(45, Some("com.notes")), approved.clone()],
        "a refused command adds nothing"
    );
    // A consent with no time (an earlier release's) adds nothing.
    let untimed = consent(&mut alice, "org:p-alice", "__withdraw_release", &args);
    let _ok = m
        .run(SystemCmd::SetReleaseApproval {
            scope: ReleaseScope::Org { org: p("alice") },
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
                "method": "__approve_release", "subject": "org:p-alice" })
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
        personal_org: None,
    };
    let args = super::account_args(&account, Some(&grant));
    let mut elsewhere = account.clone();
    elsewhere.name = "someone else".to_owned();
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
        personal: false,
        app_team: None,
        capabilities: None,
    };
    assert!(m.run_unproven(create("o2", " padded ")).is_err());
    assert!(!m.state.orgs.contains_key("o2"));
    let _ok = m.run_unproven(create("o2", "Fits")).unwrap();
    assert_eq!(m.state.orgs["o2"].name, "Fits");
}

/// The status a client sees for a refusal comes from what the log refused it
/// for, never from its wording: a quota refusal worded otherwise is still
/// `429 QUOTA_EXCEEDED`, and a refusal of another kind whose wording happens
/// to start like a quota's is still `400 REJECTED` (review finding E4).
#[test]
fn a_refusals_status_comes_from_its_kind_not_its_wording() {
    let (mut m, _alice, _bob, _carol) = proven_cluster();
    let owned = m
        .state
        .orgs
        .values()
        .filter(|o| !o.personal && o.role("alice") == Some(OrgRole::Owner))
        .count();
    m.state.quotas.max_orgs_per_account = u32::try_from(owned + 1).unwrap();
    let create = |id: &str| SystemCmd::CreateOrg {
        org_id: id.into(),
        name: "Team".into(),
        actor: "alice".into(),
        release: None,
        recovery_key: None,
        at: 0,
        name_fits: true,
        personal: false,
        app_team: None,
        capabilities: None,
    };
    let _ok = m.run_unproven(create("team")).unwrap();
    // What `apply` answers, its message reworded, as the API reads it.
    let answer = |refused: &_, wording: &str| {
        let mut out: Value = serde_json::from_slice(&super::err(refused)).unwrap();
        out["err"] = wording.into();
        let e = super::output(out.to_string().as_bytes()).unwrap_err();
        let api = crate::node::rejected(&e);
        (api.status, api.code)
    };
    let quota = m.run_unproven(create("team2")).unwrap_err();
    assert_eq!(
        answer(&quota, "too many organizations for this account"),
        (429, "QUOTA_EXCEEDED".to_owned())
    );
    let other = m
        .run_unproven(SystemCmd::DeleteOrg {
            org: "no-such-org".into(),
            actor: "alice".into(),
            consent: Value::Null,
        })
        .unwrap_err();
    assert_eq!(
        answer(&other, "quota exceeded: none, this organization is unknown"),
        (400, "REJECTED".to_owned())
    );
}

/// `CreateApp`'s own refusals reach a client with the wording, status and code
/// they always had: each check is named, none changed what it answers.
#[test]
fn a_create_apps_refusals_keep_their_wording_and_code() {
    let (mut m, _alice, _bob, _carol) = proven_cluster();
    let _prev = m.state.bundles.insert(
        "b1".into(),
        serde_json::from_value(json!({ "hash": "b1", "size": 1, "uploadedBy": "alice" })).unwrap(),
    );
    let create = |changes: Value, unit_offer: Option<PackageRef>| {
        let mut app = json!({
            "appId": "new", "name": "new", "group": 0, "bundle": "b1", "owner": "alice",
            "org": p("alice"), "replicas": [1], "createdAt": 10,
        });
        for (key, value) in changes.as_object().unwrap() {
            app[key] = value.clone();
        }
        SystemCmd::CreateApp {
            app: Box::new(serde_json::from_value(app).unwrap()),
            unit_offer,
            offers_only: false,
        }
    };
    let offer = PackageRef {
        package: "p".into(),
        version: "1.0.0".into(),
        service: None,
        compute: false,
    };
    let cases = [
        (create(json!({ "bundle": "nope" }), None), "unknown bundle"),
        (create(json!({ "appId": "app" }), None), "app exists"),
        (
            create(json!({ "deployment": "nope" }), None),
            "no such deployment",
        ),
        (
            create(json!({}), Some(offer)),
            "a unit's offer comes with the deployment that makes it",
        ),
        (
            create(json!({ "owner": "carol", "org": "org" }), None),
            "owner is not a member of the organization",
        ),
        (
            create(json!({ "owner": "bob", "org": "org" }), None),
            "creating apps here needs the CREATE_APP capability",
        ),
    ];
    for (cmd, message) in cases {
        let refused = m.run_unproven(cmd).unwrap_err();
        let api = crate::node::rejected(&refused);
        assert_eq!(
            (api.message.as_str(), api.status, api.code.as_str()),
            (message, 400, "REJECTED")
        );
    }
    m.state.quotas.max_apps_per_account = 1;
    let refused = m.run_unproven(create(json!({}), None)).unwrap_err();
    let api = crate::node::rejected(&refused);
    assert_eq!(
        (api.message.as_str(), api.status, api.code.as_str()),
        ("quota exceeded: apps", 429, "QUOTA_EXCEEDED")
    );
}

/// Every account has its personal organization from sign-up
/// (`CreateAccount { personal_org }`): it alone owns it, it is outside the
/// organization quota, and a second one is refused. An ordinary
/// organization's entry and state leave the flag out.
#[test]
fn an_account_has_one_personal_organization_outside_its_quota() {
    let (mut m, _alice, _bob, _carol) = proven_cluster();
    let mine = p("alice");
    let o = &m.state.orgs[&mine];
    assert!(o.personal);
    assert_eq!(o.name, super::PERSONAL_ORG_NAME);
    assert_eq!(o.role("alice"), Some(OrgRole::Owner));
    assert_eq!(o.members.len(), 1);
    assert_eq!(
        o.approved_releases
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        ["1.0.0"],
        "the release the account signed up through"
    );
    assert_eq!(m.state.personal_org_of("alice"), Some(&mine));

    // Alice may own one more ordinary organization than she does.
    let owned = m
        .state
        .orgs
        .values()
        .filter(|o| !o.personal && o.role("alice") == Some(OrgRole::Owner))
        .count();
    m.state.quotas.max_orgs_per_account = u32::try_from(owned + 1).unwrap();
    let create = |id: &str, actor: &str, personal: bool| SystemCmd::CreateOrg {
        org_id: id.into(),
        name: "Team".into(),
        actor: actor.into(),
        release: Some("2.3.145".into()),
        recovery_key: None,
        at: 0,
        name_fits: true,
        personal,
        app_team: None,
        capabilities: None,
    };
    let entry = serde_json::to_value(create("team", "alice", false)).unwrap();
    assert!(entry.get("personal").is_none(), "{entry}");
    let _ok = m.run_unproven(create("team", "alice", false)).unwrap();
    let refused = m.run_unproven(create("team2", "alice", false)).unwrap_err();
    assert!(refused.is_quota(), "{refused}");
    assert!(!serde_json::to_value(&m.state.orgs["team"])
        .unwrap()
        .as_object()
        .unwrap()
        .contains_key("personal"));

    // One per account.
    assert_eq!(
        m.run_unproven(create("mine2", "alice", true)),
        Err(super::PERSONAL_ORG_EXISTS)
    );

    // An app there shared with listed accounts reaches them without making
    // them members; in another organization the list reaches only members.
    let shared = |org: &str| -> crate::types::AppRecord {
        serde_json::from_value(json!({
            "appId": format!("in-{org}"), "name": "n", "group": 2, "bundle": "b",
            "owner": "alice", "org": org, "replicas": [1], "createdAt": 0,
            "grants": [{ "kind": "account", "account": "carol" }],
        }))
        .unwrap()
    };
    assert!(m.state.may_reach(&shared(&mine), Some("carol")));
    assert!(!m.state.may_reach(&shared(&mine), Some("bob")));
    assert!(!m.state.may_reach(&shared("team"), Some("carol")));

    // An app there handed to someone outside it goes to their personal
    // organization, `team` access becoming the organization's.
    let mut app = shared(&mine);
    app.app_id = "gift".into();
    app.access = crate::types::Access::team("t".into());
    let _prev = m.state.apps.insert("gift".into(), app);
    let transfer = |to: &str| SystemCmd::TransferApp {
        app_id: "gift".into(),
        to: to.into(),
        actor: "alice".into(),
        consent: Value::Null,
        owner_in_org: true,
        owner_in_team: true,
    };
    let _ok = m.run_unproven(transfer("bob")).unwrap();
    let gift = &m.state.apps["gift"];
    assert_eq!(
        (gift.owner.as_str(), gift.org.as_str()),
        ("bob", p("bob").as_str())
    );
    assert_eq!(gift.access, crate::types::Access::org());

    // An app record names its organization: one without is not read.
    let without: Result<crate::types::AppRecord, _> = serde_json::from_value(json!({
        "appId": "x", "name": "x", "group": 2, "bundle": "b", "owner": "alice",
        "replicas": [1], "createdAt": 0,
    }));
    assert!(without.is_err());

    // A state written with it reads back the same.
    let before = serde_json::to_value(&m.state).unwrap();
    let restored: SystemState = serde_json::from_value(before.clone()).unwrap();
    assert_eq!(serde_json::to_value(&restored).unwrap(), before);
}

/// From the cluster version that makes organizations ordinary
/// (`Feature::OrdinaryOrgs`, `SystemState::ordinary_orgs`) a sign-up makes no
/// organization (even one proposed below it with a personal seed), no personal one is
/// made, and one made before is an organization like any other: it counts against
/// `maxOrgsPerAccount` and its apps go only to its members. Its instances
/// still reach the accounts they name. Entries logged before the version replay as they
/// applied: a sign-up still made its personal organization then.
#[test]
fn once_organizations_are_ordinary_a_personal_one_is_like_any_other() {
    let (mut m, _alice, _bob, _carol) = proven_cluster();
    let mine = p("alice");
    // Before the version: a sign-up makes its personal organization, as it did.
    let _ok = m.run_unproven(account("dave")).unwrap();
    assert!(m.state.orgs[&p("dave")].personal);
    assert!(!m.state.ordinary_orgs());
    assert_eq!(m.state.personal_org_of("alice"), Some(&mine));

    let rc32 = "1.0.0-rc.32";
    let _ok = m
        .run_unproven(SystemCmd::SetClusterVersion {
            version: rc32.into(),
            nodes: (1..=4).map(|id| (id, rc32.to_owned())).collect(),
        })
        .unwrap();
    assert!(m.state.ordinary_orgs());
    assert_eq!(m.state.personal_org_of("alice"), None);

    // A sign-up proposed below the version (it carries a seed) and applied after it:
    // the account, and no organization.
    let _ok = m.run_unproven(account("erin")).unwrap();
    assert!(m.state.accounts.contains_key("erin"));
    assert!(!m.state.orgs.contains_key(&p("erin")));
    assert!(m.state.orgs_of("erin").is_empty());

    // No personal organization is made any more.
    let create = |id: &str, actor: &str, personal: bool| SystemCmd::CreateOrg {
        org_id: id.into(),
        name: "Team".into(),
        actor: actor.into(),
        release: None,
        recovery_key: None,
        at: 0,
        name_fits: true,
        personal,
        app_team: None,
        capabilities: None,
    };
    assert_eq!(
        m.run_unproven(create("erins", "erin", true)),
        Err(super::NO_PERSONAL_ORGS)
    );
    assert!(!m.state.orgs.contains_key("erins"));

    // The one made before counts against the quota, as any organization she owns.
    let owned = m
        .state
        .orgs
        .values()
        .filter(|o| o.role("alice") == Some(OrgRole::Owner))
        .count();
    m.state.quotas.max_orgs_per_account = u32::try_from(owned).unwrap();
    let refused = m.run_unproven(create("team", "alice", false)).unwrap_err();
    assert!(refused.is_quota(), "{refused}");
    m.state.quotas.max_orgs_per_account = u32::try_from(owned + 1).unwrap();
    let _ok = m.run_unproven(create("team", "alice", false)).unwrap();
    // Erin's first organization is an ordinary one she creates.
    let _ok = m.run_unproven(create("erins", "erin", false)).unwrap();
    assert_eq!(m.state.orgs_of("erin"), [&"erins".to_owned()]);

    // An app there goes only to a member: bob, outside it, is refused, and nothing
    // moves.
    let app: crate::types::AppRecord = serde_json::from_value(json!({
        "appId": "gift", "name": "n", "group": 2, "bundle": "b",
        "owner": "alice", "org": mine, "replicas": [1], "createdAt": 0,
        "grants": [{ "kind": "account", "account": "carol" }],
    }))
    .unwrap();
    let _prev = m.state.apps.insert("gift".into(), app);
    let transfer = SystemCmd::TransferApp {
        app_id: "gift".into(),
        to: "bob".into(),
        actor: "alice".into(),
        consent: Value::Null,
        owner_in_org: true,
        owner_in_team: true,
    };
    assert_eq!(
        m.run_unproven(transfer),
        Err(super::NEW_OWNER_NOT_A_MEMBER.into())
    );
    let gift = &m.state.apps["gift"];
    assert_eq!(
        (gift.owner.as_str(), gift.org.as_str()),
        ("alice", mine.as_str())
    );

    // Its listed accounts still reach it without being members.
    assert!(m.state.may_reach(&m.state.apps["gift"], Some("carol")));
    assert!(!m.state.may_reach(&m.state.apps["gift"], Some("bob")));
}

/// Guests (`Feature::Guests`): an account named on an instance (an `account` grant)
/// that is not a member of its organization reaches it, and the apps its unit created for
/// it, only from the logged version on and only while both the release it runs (`guests:
/// allowed`) and its organization (`allowGuests`, its owner's or an admin's with a passkey)
/// allow. Instance invites, on any instance, name the redeemer on it with the token's key (the
/// instance's access becoming grants); a manager of the instance's access mints and revokes
/// them; deleting the instance drops them.
#[test]
fn a_guest_reaches_one_instance_while_its_release_and_its_organization_allow() {
    use super::cluster_version::Feature;
    use super::{OrgSettings, APP_INVITE_GONE, GUESTS_NOT_ALLOWED};
    use crate::types::AppInvite;
    let (mut m, mut alice, _bob, _carol) = proven_cluster();
    let _ok = m.run_unproven(account("erin")).unwrap();
    for (package, guests) in [("g", json!("allowed")), ("f", Value::Null)] {
        let mut release = json!({
            "version": "1.0.0", "mpk": format!("{package}-mpk"), "mpkSize": 1, "source": "upload",
            "services": { "": format!("b-{package}") }, "installedBy": "alice", "installedAt": 5,
        });
        if !guests.is_null() {
            release["guests"] = guests;
        }
        let cmd = SystemCmd::InstallPackage {
            package: package.into(),
            signer_id: format!("did:key:{package}"),
            release: serde_json::from_value(release).unwrap(),
        };
        let _ok = m.run_unproven(cmd).unwrap();
    }
    let app = |id: &str, package: &str, access: Value, links: Value| -> crate::types::AppRecord {
        serde_json::from_value(json!({
            "appId": id, "name": id, "group": 2, "bundle": format!("b-{package}"), "owner": "alice",
            "org": "org", "replicas": [1], "createdAt": 0, "grants": access, "links": links,
            "release": { "package": package, "version": "1.0.0" },
        }))
        .unwrap()
    };
    let listed = json!([{ "kind": "account", "account": "carol" }]);
    for a in [
        app("open", "g", listed.clone(), json!({ "kv": "member" })),
        app("closed", "f", listed.clone(), json!({})),
        app("member", "g", json!([{ "kind": "org" }]), json!({})),
        app("wide", "g", json!([{ "kind": "org" }]), json!({})),
    ] {
        let _prev = m.state.apps.insert(a.app_id.clone(), a);
    }
    let reaches =
        |m: &SystemMachine, id: &str, who: &str| m.state.may_reach(&m.state.apps[id], Some(who));

    // Before the version, a listed outsider reaches nothing, as before.
    assert!(!reaches(&m, "open", "carol"));
    let rc33 = "1.0.0-rc.33";
    let _ok = m
        .run_unproven(SystemCmd::SetClusterVersion {
            version: rc33.into(),
            nodes: (1..=4).map(|id| (id, rc33.to_owned())).collect(),
        })
        .unwrap();
    assert!(m.state.guests_on());
    // The release allows guests: carol reaches it and its unit's member, not the rest.
    assert!(reaches(&m, "open", "carol"));
    assert!(m.state.is_guest(&m.state.apps["open"], "carol"));
    assert!(
        reaches(&m, "member", "carol"),
        "a unit's member, by its instance's list"
    );
    assert!(
        !reaches(&m, "closed", "carol"),
        "a release that says nothing forbids them"
    );
    assert!(!reaches(&m, "wide", "carol"));
    assert!(!m.state.manages_grants(&m.state.apps["open"], "carol"));

    // The organization's switch: its owner, with a passkey, once; a member is refused.
    let settings = |allow: bool, actor: &str, consent: Value| SystemCmd::SetOrgSettings {
        org: "org".into(),
        settings: OrgSettings {
            allow_guests: Some(allow),
            allow_public_links: None,
        },
        actor: actor.into(),
        consent,
    };
    assert_eq!(
        settings(false, "alice", Value::Null).features(),
        [Feature::Guests]
    );
    assert!(m.run_unproven(settings(false, "bob", Value::Null)).is_err());
    needs_consent(
        &mut m,
        &mut alice,
        ("org:org", "__set_org_settings"),
        &json!({ "allowGuests": false }),
        &json!({ "allowGuests": true }),
        |c| settings(false, "alice", c),
    );
    assert!(!reaches(&m, "open", "carol"), "off: refused at once");
    assert!(!reaches(&m, "member", "carol"));
    let _ok = m
        .run_unproven(settings(true, "alice", Value::Null))
        .unwrap();
    assert!(reaches(&m, "open", "carol"));

    // Instance invites: a manager of the instance's access mints one, while it takes
    // guests and has a list; the redemption carries the token key's signature.
    let token = "inv_guest";
    let key = crate::proof::invite_key(token);
    let invite = |app: &str, uses: u32| AppInvite {
        app: app.into(),
        expires_at: 1_000,
        uses_left: uses,
        created_by: String::new(),
        created_at: 10,
        scope: None,
    };
    let create = |key: &str, app: &str, actor: &str, consent: Value| SystemCmd::CreateAppInvite {
        invite_key: key.into(),
        invite: invite(app, 1),
        actor: actor.into(),
        consent,
    };
    assert_eq!(
        create(&key, "open", "alice", Value::Null).features(),
        [Feature::Guests, Feature::AccessGrants]
    );
    assert!(m
        .run_unproven(create(&key, "open", "bob", Value::Null))
        .is_err());
    // Any instance takes one, not only a list, whatever its switches say: whether guests are
    // allowed is judged when someone redeems it.
    let closed_token = "inv_closed";
    let closed_key = crate::proof::invite_key(closed_token);
    let mut closed = create(&closed_key, "closed", "alice", Value::Null);
    if let SystemCmd::CreateAppInvite { invite, .. } = &mut closed {
        invite.uses_left = 2;
    }
    let _ok = m.run_unproven(closed).unwrap();
    let redeem_as = |key: &str, token: &str, who: &str| SystemCmd::RedeemAppInvite {
        invite_key: key.into(),
        account: who.into(),
        now: 20,
        signature: crate::proof::sign_invite_redeem(token, who, 20),
    };
    // Its release forbids guests: an outsider is refused, a member of its organization is
    // named on it.
    assert_eq!(
        m.run(redeem_as(&closed_key, closed_token, "erin")),
        Err(GUESTS_NOT_ALLOWED)
    );
    assert!(!reaches(&m, "closed", "erin"));
    let out = m.run(redeem_as(&closed_key, closed_token, "bob")).unwrap();
    assert_eq!(
        out,
        json!({ "app": "closed", "org": "org", "member": true })
    );
    assert!(reaches(&m, "closed", "bob"));
    assert!(m.state.apps["closed"].access.names("bob"));
    let _ok = m
        .run_unproven(create("k-wide", "wide", "alice", Value::Null))
        .unwrap();
    let args = super::app_invite_args(&key, &invite("open", 1));
    needs_consent(
        &mut m,
        &mut alice,
        ("open", "__create_app_invite"),
        &args,
        &json!({ "inviteKey": key, "expiresAt": 1, "uses": 1 }),
        |c| create(&key, "open", "alice", c),
    );
    assert_eq!(m.state.app_invites[&key].created_by, "alice");
    let redeem = |signature: String| SystemCmd::RedeemAppInvite {
        invite_key: key.clone(),
        account: "erin".into(),
        now: 20,
        signature,
    };
    assert!(
        m.run(redeem("00".repeat(64))).is_err(),
        "a forged redemption"
    );
    let signed = crate::proof::sign_invite_redeem(token, "erin", 20);
    let out = m.run(redeem(signed.clone())).unwrap();
    assert_eq!(out, json!({ "app": "open", "org": "org", "member": false }));
    assert!(reaches(&m, "open", "erin"));
    assert!(reaches(&m, "open", "carol"), "the list it had is kept");
    assert!(
        m.state.orgs["org"].role("erin").is_none(),
        "a guest, not a member"
    );
    assert!(!m.state.app_invites.contains_key(&key), "its one use spent");
    assert!(m.run(redeem(signed)).is_err());
    assert_eq!(
        m.run_unproven(redeem(String::new())),
        Err(APP_INVITE_GONE.into())
    );

    // The organization's switch turned off after an invite was made: an outsider's
    // redemption is refused on the state it applies to, and the invite stays.
    let off_token = "inv_off";
    let off_key = crate::proof::invite_key(off_token);
    let _ok = m
        .run_unproven(create(&off_key, "open", "alice", Value::Null))
        .unwrap();
    let _ok = m.run_unproven(account("frank")).unwrap();
    let _ok = m
        .run_unproven(settings(false, "alice", Value::Null))
        .unwrap();
    assert_eq!(
        m.run(redeem_as(&off_key, off_token, "frank")),
        Err(GUESTS_NOT_ALLOWED)
    );
    assert!(!reaches(&m, "open", "frank"));
    assert!(m.state.app_invites.contains_key(&off_key));
    let _ok = m
        .run_unproven(settings(true, "alice", Value::Null))
        .unwrap();
    let _ok = m.run(redeem_as(&off_key, off_token, "frank")).unwrap();
    assert!(reaches(&m, "open", "frank"));

    // Revoked by its creator; dropped with its instance.
    let _ok = m
        .run_unproven(create("k2", "open", "alice", Value::Null))
        .unwrap();
    let _ok = m
        .run_unproven(SystemCmd::RevokeAppInvite {
            app_id: "open".into(),
            invite_key: "k2".into(),
            actor: "alice".into(),
            consent: Value::Null,
        })
        .unwrap();
    assert!(!m.state.app_invites.contains_key("k2"));
    let _ok = m
        .run_unproven(create("k3", "open", "alice", Value::Null))
        .unwrap();
    m.state.remove_app("open");
    m.state.remove_app("wide");
    m.state.remove_app("closed");
    assert!(m.state.app_invites.is_empty());

    // A state without invites or the switch writes as it did.
    let o = serde_json::to_value(&m.state.orgs["org"]).unwrap();
    assert!(o.get("allowGuests").is_none(), "{o}");
    let st = serde_json::to_value(&m.state).unwrap();
    assert!(st.get("appInvites").is_none());
}

/// Public links (`Feature::PublicLinks`): whoever may change an instance's access makes one
/// with their passkey over its terms, while the organization allows public links; a link
/// admits visitors only from the logged version on, until it expires, is revoked, its
/// organization turns links off, or its instance goes. Only the token's key is logged.
#[test]
fn a_public_link_admits_visitors_while_it_and_its_organization_allow() {
    use super::cluster_version::Feature;
    use super::{OrgSettings, PUBLIC_LINKS_OFF, PUBLIC_LINK_GONE};
    use crate::types::PublicLink;
    let (mut m, mut alice, _bob, _carol) = proven_cluster();
    let _prev = m.state.apps.insert(
        "forms".into(),
        serde_json::from_value(json!({
            "appId": "forms", "name": "forms", "group": 2, "bundle": "b", "owner": "alice",
            "org": "org", "replicas": [1], "createdAt": 0, "grants": [{ "kind": "org" }],
        }))
        .unwrap(),
    );
    let key = crate::proof::public_link_key("pl_token");
    let link = |expires_at: Option<u64>| PublicLink {
        app: "forms".into(),
        label: Some("Survey".into()),
        expires_at,
        rate_per_minute: 30,
        created_by: String::new(),
        created_at: 10,
    };
    let create = |key: &str, actor: &str, consent: Value, expires_at: Option<u64>| {
        SystemCmd::CreatePublicLink {
            link_key: key.into(),
            link: link(expires_at),
            actor: actor.into(),
            consent,
        }
    };
    assert_eq!(
        create(&key, "alice", Value::Null, None).features(),
        [Feature::AccessGrants, Feature::PublicLinks]
    );
    // A member who may not change the instance's access makes none.
    assert!(m
        .run_unproven(create(&key, "bob", Value::Null, None))
        .is_err());
    needs_consent(
        &mut m,
        &mut alice,
        ("forms", "__create_public_link"),
        &super::public_link_args(&key, &link(None)),
        &json!({ "linkKey": key, "label": null, "expiresAt": null, "ratePerMinute": 30 }),
        |c| create(&key, "alice", c, None),
    );
    // The link is one more grant of the instance, beside its `org` grant; it admits no
    // account.
    let forms = &m.state.apps["forms"];
    assert_eq!(forms.access.link(&key).unwrap().created_by, "alice");
    assert_eq!(forms.access.grants()[0], crate::types::Grant::Org);
    assert!(m.state.may_reach(forms, Some("bob")));
    assert!(!m.state.may_reach(forms, None));
    // Setting the grants keeps the link as it is; a link it does not hold is never made
    // that way.
    let set = |access: Value| SystemCmd::SetAccess {
        app_id: "forms".into(),
        access: serde_json::from_value(access).unwrap(),
        actor: "alice".into(),
        owner_in_org: true,
        owner_in_team: true,
    };
    let _ok = m.run_unproven(set(json!([]))).unwrap();
    assert!(m.state.apps["forms"].access.link(&key).is_some());
    let forged = json!([{ "kind": "link", "link": "forged",
        "ratePerMinute": 600, "createdBy": "alice", "createdAt": 0 }]);
    assert_eq!(
        m.run_unproven(set(forged)),
        Err(
            "a public link is made with its own consent, not by setting the instance's access"
                .to_owned()
                .into()
        )
    );
    let _ok = m.run_unproven(set(json!([{ "kind": "org" }]))).unwrap();
    assert_eq!(m.state.apps["forms"].access.links().len(), 1);
    // Below the version it admits nobody (no visitor is ever written below it).
    assert!(m.state.public_link_admits(&key, 20).is_none());
    let rc33 = "1.0.0-rc.33";
    let _ok = m
        .run_unproven(SystemCmd::SetClusterVersion {
            version: rc33.into(),
            nodes: (1..=4).map(|id| (id, rc33.to_owned())).collect(),
        })
        .unwrap();
    assert!(m.state.public_link_admits(&key, 20).is_some());

    // The organization turns links off: no new one, and the existing one admits nobody.
    let settings = |allow: bool| SystemCmd::SetOrgSettings {
        org: "org".into(),
        settings: OrgSettings {
            allow_guests: None,
            allow_public_links: Some(allow),
        },
        actor: "alice".into(),
        consent: Value::Null,
    };
    assert_eq!(settings(false).features(), [Feature::PublicLinks]);
    let _ok = m.run_unproven(settings(false)).unwrap();
    assert!(m.state.public_link_admits(&key, 20).is_none());
    assert_eq!(
        m.run_unproven(create("other", "alice", Value::Null, None)),
        Err(PUBLIC_LINKS_OFF.into())
    );
    let _ok = m.run_unproven(settings(true)).unwrap();
    assert!(m.state.public_link_admits(&key, 20).is_some());

    // An expired link admits nobody.
    let _ok = m
        .run_unproven(create("soon", "alice", Value::Null, Some(100)))
        .unwrap();
    assert!(m.state.public_link_admits("soon", 99).is_some());
    assert!(m.state.public_link_admits("soon", 100).is_none());

    // Revoked by its creator with a passkey; gone after.
    let revoke = |c: Value| SystemCmd::RevokePublicLink {
        app_id: "forms".into(),
        link_key: key.clone(),
        actor: "alice".into(),
        consent: c,
    };
    needs_consent(
        &mut m,
        &mut alice,
        ("forms", "__revoke_public_link"),
        &json!({ "linkKey": key }),
        &json!({ "linkKey": "other" }),
        revoke,
    );
    assert!(m.state.public_link_admits(&key, 20).is_none());
    assert_eq!(
        m.run_unproven(revoke(Value::Null)),
        Err(PUBLIC_LINK_GONE.into())
    );
    // Dropped with its instance.
    m.state.remove_app("forms");
    assert_eq!(m.state.public_link_count(), 0);
    let st = serde_json::to_value(&m.state).unwrap();
    assert!(st.get("publicLinks").is_none());
    let o = serde_json::to_value(&m.state.orgs["org"]).unwrap();
    assert!(o.get("allowPublicLinks").is_none(), "{o}");
}

/// A visitor's proof is its own key's signature over the link, its key and a time near
/// the node's clock: another key's, another link's, a stale or a future one are refused.
#[test]
fn a_visitors_proof_names_its_link_key_and_time() {
    use crate::proof::{public_link_key, sign_visitor, verify_visitor, VISITOR_PROOF_WINDOW_SECS};
    let link = public_link_key("pl_token");
    assert_ne!(
        link,
        crate::proof::invite_key("pl_token"),
        "a link is not an invite"
    );
    let (key, sig) = sign_visitor(&[9; 32], &link, 1_000);
    assert!(verify_visitor(&link, &key, 1_000, &sig, 1_000).is_ok());
    assert!(verify_visitor(&link, &key, 1_000, &sig, 1_000 + VISITOR_PROOF_WINDOW_SECS).is_ok());
    assert!(verify_visitor(&link, &key, 1_000, &sig, 1_001 + VISITOR_PROOF_WINDOW_SECS).is_err());
    assert!(
        verify_visitor(&link, &key, 1_001, &sig, 1_000).is_err(),
        "another time"
    );
    let other = public_link_key("pl_other");
    assert!(
        verify_visitor(&other, &key, 1_000, &sig, 1_000).is_err(),
        "another link"
    );
    let (key2, _) = sign_visitor(&[8; 32], &link, 1_000);
    assert!(
        verify_visitor(&link, &key2, 1_000, &sig, 1_000).is_err(),
        "another key"
    );
    assert!(verify_visitor(&link, &key.to_uppercase(), 1_000, &sig, 1_000).is_err());
}

/// An instance's access is a list of grants: a team, named members and guests together ("team
/// t plus a partner"), each judged on its own; a guest's grant carries the app's scope; a list
/// that cannot stand is refused where it applies. An invite names its redeemer on any
/// instance: a guest, or a named member when they are a member, with the invite's scope.
/// Deleting a team takes the instances only it admitted and its grant from the rest.
#[test]
fn an_instances_access_is_a_list_of_grants() {
    use super::cluster_version::Feature;
    use crate::types::{Access, AppInvite, Grant};
    let (mut m, _alice, _bob, _carol) = proven_cluster();
    for who in ["dan", "erin"] {
        let _ok = m.run_unproven(account(who)).unwrap();
    }
    let release = json!({
        "version": "1.0.0", "mpk": "g-mpk", "mpkSize": 1, "source": "upload",
        "services": { "": "b-g" }, "installedBy": "alice", "installedAt": 5, "guests": "allowed",
    });
    let _ok = m
        .run_unproven(SystemCmd::InstallPackage {
            package: "g".into(),
            signer_id: "did:key:g".into(),
            release: serde_json::from_value(release).unwrap(),
        })
        .unwrap();
    let rc33 = "1.0.0-rc.33";
    let _ok = m
        .run_unproven(SystemCmd::SetClusterVersion {
            version: rc33.into(),
            nodes: (1..=4).map(|id| (id, rc33.to_owned())).collect(),
        })
        .unwrap();
    {
        let o = m.state.orgs.get_mut("org").unwrap();
        let _prev = o
            .members
            .insert("dan".into(), crate::types::OrgRole::Member);
        let mut u = o.teams["t"].clone();
        u.team_id = "u".into();
        u.members.clear();
        let _prev = o.teams.insert("u".into(), u);
        let _lead = o
            .teams
            .get_mut("t")
            .unwrap()
            .members
            .insert("bob".into(), false);
    }
    for id in ["x", "y", "z", "w"] {
        let record = json!({
            "appId": id, "name": id, "group": 2, "bundle": "b-g", "owner": "alice",
            "org": "org", "replicas": [1], "createdAt": 0, "grants": [],
            "release": { "package": "g", "version": "1.0.0" },
        });
        let _prev = m
            .state
            .apps
            .insert(id.into(), serde_json::from_value(record).unwrap());
    }
    let set = |app: &str, grants: Value| SystemCmd::SetAccess {
        app_id: app.into(),
        access: serde_json::from_value(grants).unwrap(),
        actor: "alice".into(),
        owner_in_org: true,
        owner_in_team: true,
    };
    let partners = json!({ "channel": "partners" });
    let x = set(
        "x",
        json!([{ "kind": "team", "team": "t" },
            { "kind": "account", "account": "carol", "scope": partners },
            { "kind": "account", "account": "dan" }]),
    );
    assert_eq!(x.features(), [Feature::AccessGrants]);
    let _ok = m.run_unproven(x).unwrap();
    let reaches =
        |m: &SystemMachine, id: &str, who: Option<&str>| m.state.may_reach(&m.state.apps[id], who);
    // The team, a named member outside it, a guest; not another outsider, nor anyone signed
    // out, nor the owner outside the team (an owner is judged by the teams, as on `team`).
    for (who, want) in [
        (Some("alice"), false),
        (Some("bob"), true),
        (Some("dan"), true),
        (Some("carol"), true),
        (Some("erin"), false),
        (None, false),
    ] {
        assert_eq!(reaches(&m, "x", who), want, "{who:?}");
    }
    let xa = &m.state.apps["x"];
    assert!(m.state.is_guest(xa, "carol") && !m.state.is_guest(xa, "dan"));
    assert_eq!(xa.access.scope_of("carol"), Some(&partners));
    assert_eq!(xa.access.the_team().map(String::as_str), Some("t"));
    // Anyone: every signed-in account, a member or not, never a caller without one.
    let _ok = m
        .run_unproven(set("w", json!([{ "kind": "anyone" }])))
        .unwrap();
    assert!(!reaches(&m, "w", None), "signed out: refused");
    assert!(reaches(&m, "w", Some("erin")), "a signed-in outsider");
    assert!(reaches(&m, "w", Some("dan")), "a member");
    assert_eq!(
        serde_json::to_value(&m.state.apps["w"]).unwrap()["grants"],
        json!([{ "kind": "anyone" }])
    );

    // Lists that cannot stand.
    let big = json!({ "note": "x".repeat(300) });
    for (bad, why) in [
        (
            json!([{ "kind": "org" }, { "kind": "team", "team": "t" }]),
            "a grant to the organization covers its teams: not both",
        ),
        (
            json!([{ "kind": "team", "team": "t" }, { "kind": "team", "team": "t" }]),
            "a grant is listed twice",
        ),
        (
            json!([{ "kind": "account", "account": "dan" },
                { "kind": "account", "account": "dan", "scope": 1 }]),
            "a grant is listed twice",
        ),
        (
            json!([{ "kind": "team", "team": "nope" }]),
            "no such team in the app's organization",
        ),
        (
            json!([{ "kind": "account", "account": "nobody" }]),
            "unknown account",
        ),
        (
            json!([{ "kind": "account", "account": "carol", "scope": big }]),
            "a grant's scope is at most 256 bytes of JSON",
        ),
    ] {
        assert_eq!(m.run_unproven(set("y", bad)), Err(why.into()));
    }
    assert!(
        serde_json::from_value::<Grant>(json!({ "kind": "team", "team": "t", "extra": 1 }))
            .is_err(),
        "closed input"
    );

    // An invite on a `team` instance, with a scope: a guest is named with it, a member too.
    let _ok = m
        .run_unproven(SystemCmd::SetAccess {
            app_id: "y".into(),
            access: Access::team("t".into()),
            actor: "alice".into(),
            owner_in_org: true,
            owner_in_team: true,
        })
        .unwrap();
    for token in ["inv_a", "inv_b"] {
        let _ok = m
            .run_unproven(SystemCmd::CreateAppInvite {
                invite_key: crate::proof::invite_key(token),
                invite: AppInvite {
                    app: "y".into(),
                    expires_at: 1_000,
                    uses_left: 1,
                    created_by: String::new(),
                    created_at: 10,
                    scope: Some(partners.clone()),
                },
                actor: "alice".into(),
                consent: Value::Null,
            })
            .unwrap();
    }
    let redeem = |token: &str, who: &str| SystemCmd::RedeemAppInvite {
        invite_key: crate::proof::invite_key(token),
        account: who.into(),
        now: 20,
        signature: crate::proof::sign_invite_redeem(token, who, 20),
    };
    assert!(!reaches(&m, "y", Some("erin")));
    let out = m.run(redeem("inv_a", "erin")).unwrap();
    assert_eq!(out, json!({ "app": "y", "org": "org", "member": false }));
    let out = m.run(redeem("inv_b", "dan")).unwrap();
    assert_eq!(out, json!({ "app": "y", "org": "org", "member": true }));
    let ya = &m.state.apps["y"];
    assert_eq!(
        ya.access.grants(),
        [
            Grant::Team { team: "t".into() },
            Grant::Account {
                account: "erin".into(),
                scope: Some(partners.clone())
            },
            Grant::Account {
                account: "dan".into(),
                scope: Some(partners.clone())
            },
        ]
    );
    assert!(reaches(&m, "y", Some("erin")) && reaches(&m, "y", Some("dan")));
    assert!(reaches(&m, "y", Some("bob")), "the team keeps its way in");

    // Deleting team u: z (t and u) keeps t; w2 (only u, and a guest) goes with it.
    let _ok = m
        .run_unproven(set(
            "z",
            json!([{ "kind": "team", "team": "t" }, { "kind": "team", "team": "u" }]),
        ))
        .unwrap();
    let _ok = m
        .run_unproven(set(
            "w",
            json!([{ "kind": "team", "team": "u" }, { "kind": "account", "account": "carol" }]),
        ))
        .unwrap();
    assert_eq!(m.state.team_apps("org", "u"), ["w"]);
    let _ok = m
        .run_unproven(SystemCmd::DeleteTeam {
            org: "org".into(),
            team: "u".into(),
            actor: "alice".into(),
            consent: Value::Null,
            apps_need_consent: false,
            keep_apps: false,
        })
        .unwrap();
    assert!(!m.state.apps.contains_key("w"));
    assert_eq!(
        m.state.apps["z"].access.grants(),
        [Grant::Team { team: "t".into() }]
    );
}

/// Deleting a team with `keep_apps` keeps the instances only it admitted: each loses the team's
/// grant and keeps the rest (a guest), or with nothing left is its owner's alone (an empty
/// grant list, a `team` mode included); an instance another team admits loses only the grant,
/// as without the flag. The flag is a feature of its own, stated in the deleter's consent; an
/// entry without it deletes them, as before.
#[test]
fn a_team_deleted_keeping_its_instances_leaves_them_their_other_grants() {
    use super::cluster_version::Feature;
    use crate::types::{Access, Grant};
    let (mut m, mut alice, _bob, _carol) = proven_cluster();
    let _ok = m.run_unproven(account("erin")).unwrap();
    let release = json!({
        "version": "1.0.0", "mpk": "g-mpk", "mpkSize": 1, "source": "upload",
        "services": { "": "b-g" }, "installedBy": "alice", "installedAt": 5, "guests": "allowed",
    });
    let _ok = m
        .run_unproven(SystemCmd::InstallPackage {
            package: "g".into(),
            signer_id: "did:key:g".into(),
            release: serde_json::from_value(release).unwrap(),
        })
        .unwrap();
    let rc33 = "1.0.0-rc.33";
    let _ok = m
        .run_unproven(SystemCmd::SetClusterVersion {
            version: rc33.into(),
            nodes: (1..=4).map(|id| (id, rc33.to_owned())).collect(),
        })
        .unwrap();
    {
        let o = m.state.orgs.get_mut("org").unwrap();
        for id in ["u", "v"] {
            let mut team = o.teams["t"].clone();
            team.team_id = id.into();
            let _prev = o.teams.insert(id.into(), team);
        }
    }
    let grants = |list: Value| -> Access { serde_json::from_value(list).unwrap() };
    let guest = json!({ "kind": "account", "account": "erin" });
    for (id, access) in [
        // Only u, and a guest.
        (
            "g",
            grants(json!([{ "kind": "team", "team": "u" }, guest.clone()])),
        ),
        // Only u.
        ("e", grants(json!([{ "kind": "team", "team": "u" }]))),
        // u alone.
        ("m", Access::team("u".into())),
        // u and t: keeps t either way.
        (
            "k",
            grants(json!([{ "kind": "team", "team": "u" }, { "kind": "team", "team": "t" }])),
        ),
        // Only v: deleted with v (the flag absent).
        ("d", grants(json!([{ "kind": "team", "team": "v" }]))),
    ] {
        let record = json!({
            "appId": id, "name": id, "group": 2, "bundle": "b-g", "owner": "alice",
            "org": "org", "replicas": [1], "createdAt": 0, "grants": access,
            "release": { "package": "g", "version": "1.0.0" },
        });
        let _prev = m
            .state
            .apps
            .insert(id.into(), serde_json::from_value(record).unwrap());
    }
    let delete = |team: &str, keep_apps: bool, consent: Value| SystemCmd::DeleteTeam {
        org: "org".into(),
        team: team.into(),
        actor: "alice".into(),
        consent,
        apps_need_consent: true,
        keep_apps,
    };
    assert_eq!(
        delete("u", true, Value::Null).features(),
        [Feature::KeepTeamApps]
    );
    assert!(delete("u", false, Value::Null).features().is_empty());
    assert_eq!(
        serde_json::to_value(delete("u", false, Value::Null)).unwrap()["keep_apps"],
        Value::Null,
        "an entry without the flag is written as before"
    );
    assert_eq!(m.state.team_apps("org", "u"), ["e", "g", "m"]);
    // The consent states the flag: one over a plain deletion keeps nothing.
    let keep_args = super::delete_team_args("u", &["e".into(), "g".into(), "m".into()], true);
    assert_eq!(keep_args["keepApps"], json!(true));
    let plain = super::delete_team_args("u", &["e".into(), "g".into(), "m".into()], false);
    assert!(m.run(delete("u", true, Value::Null)).is_err(), "no consent");
    let wrong = consent(&mut alice, "org:org", "__delete_team", &plain);
    assert!(
        m.run(delete("u", true, wrong)).is_err(),
        "a plain deletion's"
    );
    let real = consent(&mut alice, "org:org", "__delete_team", &keep_args);
    let out = m.run(delete("u", true, real)).unwrap();
    assert_eq!(out["keptApps"], json!(["e", "g", "m"]));
    assert_eq!(out["apps"], json!([]));
    let reaches =
        |m: &SystemMachine, id: &str, who: &str| m.state.may_reach(&m.state.apps[id], Some(who));
    assert!(!m.state.orgs["org"].teams.contains_key("u"));
    assert_eq!(m.state.apps["g"].access, grants(json!([guest])));
    assert!(reaches(&m, "g", "erin"), "the guest keeps their way in");
    assert!(reaches(&m, "g", "alice"), "and the owner");
    for id in ["e", "m"] {
        assert_eq!(m.state.apps[id].access, grants(json!([])), "{id}");
        assert!(reaches(&m, id, "alice"), "the owner alone: {id}");
        assert!(!reaches(&m, id, "bob"), "{id}");
    }
    assert_eq!(
        m.state.apps["k"].access.grants(),
        [Grant::Team { team: "t".into() }]
    );
    // The default deletes them, as an entry an earlier release logged does.
    let logged: SystemCmd = serde_json::from_value(json!({ "op": "deleteTeam", "org": "org",
        "team": "v", "actor": "alice" }))
    .unwrap();
    let out = m.run_unproven(logged).unwrap();
    assert_eq!(out["apps"], json!(["d"]));
    assert!(!m.state.apps.contains_key("d"));
}

/// From the current-rules mark on, a `CreateOrg` without `name_fits` is
/// judged as the API logs it now: a name that does not fit is refused.
/// Before the mark it replays taking any name.
#[test]
fn a_create_org_after_the_current_rules_mark_needs_a_name_that_fits() {
    let (mut m, _alice, _bob, _carol) = proven_cluster();
    let old = |id: &str| -> SystemCmd {
        serde_json::from_value(json!({
            "op": "createOrg", "org_id": id, "name": " padded ", "actor": "alice"
        }))
        .unwrap()
    };
    m.index = 10;
    let _ok = m.run(old("before")).unwrap();
    assert_eq!(m.state.orgs["before"].name, " padded ");
    m.index = 11;
    let _ok = m.run(SystemCmd::RequireCurrentRules).unwrap();
    m.index = 12;
    assert!(m.run(old("after")).is_err());
    assert!(!m.state.orgs.contains_key("after"));
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
        clamp_grants: false,
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
        clamp_grants: true,
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
            "owner": "alice", "org": "org", "grants": [{ "kind": "team", "team": "c" }],
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
        keep_apps: false,
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
        keep_apps: false,
    });
    assert_eq!(deletion["apps_need_consent"], json!(true), "{deletion}");
}

/// From the `RequireCurrentRules` mark on, team commands apply under this
/// release's rules whatever flags the entry carries: a removal takes the
/// rows below, an invite names only teams its inviter sees, a deletion
/// that takes apps needs the passkey, and a redemption that changes
/// nothing spends no use.
#[test]
fn team_commands_after_the_current_rules_mark_apply_under_them() {
    let (mut m, mut alice, mut bob) = nested_teams();
    m.index = 10;
    let _ok = m.run(SystemCmd::RequireCurrentRules).unwrap();
    m.index = 11;
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
        !m.state.orgs["org"].in_team("bob", "c"),
        "the row below goes too"
    );

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
    let key = crate::proof::invite_key("inv_old");
    let invite: crate::types::Invite = serde_json::from_value(json!({
        "org": "org", "role": "member", "teams": ["x"], "expiresAt": 100, "usesLeft": 1,
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
    assert_eq!(
        m.run(old).unwrap_err(),
        "an invite names only teams the inviter sees"
    );

    let old: SystemCmd = serde_json::from_value(json!({
        "op": "deleteTeam", "org": "org", "team": "t", "actor": "alice",
    }))
    .unwrap();
    assert_eq!(m.run(old).unwrap_err(), DELETE_TEAM_NEEDS_CONSENT);
    assert!(m.state.apps.contains_key("inapp"));

    keep_invite(&mut m, "inv_link", "member", &[], 1);
    let key = crate::proof::invite_key("inv_link");
    let old: SystemCmd = serde_json::from_value(json!({
        "op": "redeemInvite",
        "invite_key": &key,
        "account": "alice",
        "now": 10,
        "signature": crate::proof::sign_invite_redeem("inv_link", "alice", 10),
    }))
    .unwrap();
    let _ok = m.run(old).unwrap();
    assert_eq!(m.state.invites[&key].uses_left, 1, "left for the invitee");
}

/// An account an organization's app names (its owner, an `account`
/// grant) reaches it, and receives its events, only while a member.
#[test]
fn an_org_app_is_reached_only_by_current_members() {
    let (mut m, _alice, _bob, _carol) = proven_cluster();
    let listed: AppRecord = serde_json::from_value(json!({
        "appId": "dm", "name": "dm", "group": 4, "bundle": "b1", "owner": "bob",
        "org": "org", "grants": [{ "kind": "account", "account": "alice" }, { "kind": "account", "account": "bob" }, { "kind": "account", "account": "carol" }],
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
    // In its owner's personal organization, the list alone decides.
    let personal = AppRecord {
        org: p("bob"),
        ..listed
    };
    assert!(m.state.may_reach(&personal, Some("carol")));
}

/// An instance only named people reach (a DM, a private channel) of a package with an
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
            "grants": [{ "kind": "account", "account": "alice" }, { "kind": "account", "account": "bob" }, { "kind": "account", "account": "carol" }, { "kind": "account", "account": "dave" }],
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
    // No app team for its package: the list alone, as before.
    let raw = dm("raw", "b1");
    assert!(s.may_reach(&raw, Some("carol")) && s.may_reach(&raw, Some("dave")));

    // Moving the DM onto Chat's team gives rows to who reaches it: Alice and Bob.
    let scope = SystemCmd::ScopeApps {
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
    let mut m = fresh();
    let _ok = m.run_authorized(scope).unwrap();
    assert_eq!(rows(&m), ["alice", "bob"]);
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
            "org": "org", "grants": [{ "kind": "org" }], "replicas": [1, 2, 3],
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
        member_child_teams: true,
        all_apps_rows: true,
        by_release: flag,
    };
    // The team each instance is on after the step.
    let teams = |m: &SystemMachine| -> Vec<Option<String>> {
        ["c1", "n1"]
            .map(|id| {
                let team = m.state.apps[id].access.the_team()?;
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
        assert_eq!(m.run_authorized(channel(who, who)), Err(refused.into()));
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
        assert_eq!(m.run_authorized(channel(&id, who)), Err(refused.into()));
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
        Err(super::ONLY_ADMINS_ALL_APPS.into())
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
        Err(super::ONLY_ADMINS_ALL_APPS.into())
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
                "owner": "alice", "org": "org", "grants": [{ "kind": "org" }],
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
        let team = out["grants"][0]["team"].as_str().unwrap();
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
        Err(refused.into()),
        "as logged before"
    );
    assert_eq!(
        m.run_authorized(create("open", "bob", TeamJoin::Everyone, true)),
        Err(refused.into())
    );
    assert_eq!(
        m.run_authorized(create("c", "carol", TeamJoin::Invite, true)),
        Err(refused.into()),
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
        Err(refused.into())
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
        Err(refused.into())
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
        Err("not allowed to manage this team".into())
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
        assert_eq!(m.run_authorized(set("alice", teams)), Err(why.into()));
    }
    // Not a member of the organization: no row anywhere.
    let mut outsider = set("alice", vec![row("chat", None)]);
    if let SystemCmd::SetTeamMembers { account, .. } = &mut outsider {
        *account = "erin".into();
    }
    assert_eq!(
        m.run_authorized(outsider),
        Err("only organization members join teams".into())
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
        Err("not allowed to manage this team".into())
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
            "org": "org", "grants": [], "replicas": [1, 2, 3], "createdAt": 0
        }))
        .unwrap(),
    );
    (m, alice, bob)
}

fn set_access(app: &str, actor: &str) -> SystemCmd {
    SystemCmd::SetAccess {
        app_id: app.into(),
        access: Access::new(vec![crate::types::Grant::Anyone]),
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
        members: None,
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
            "org": "org", "grants": [{ "kind": "team", "team": "pt" }],
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
        org_quotas: true,
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
    assert_eq!(m.state.apps["oa"].org.as_str(), "new");
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
    assert_eq!(app.org.as_str(), "new");
    let Some(team) = app.access.the_team() else {
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
    let team = out["grants"][0]["team"].as_str().unwrap().to_owned();
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
        assert_eq!(m.state.apps["oa"].org.as_str(), "org", "{actor}");
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
    assert_eq!(m.state.apps["oa"].org.as_str(), "new");
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
        a.org = "new".into();
        a.access = crate::types::Access::org();
        a.owner = "alice".into();
        a
    });
    m.state.set_app_file_bytes("na", 60);
    let e = m
        .run_unproven(move_app("oa", "new", "alice", true, Value::Null))
        .unwrap_err();
    assert!(e.is_quota(), "{e}");
    m.state.set_app_file_bytes("na", 50);

    m.state.quotas.max_concurrent_runs_per_org = 1;
    let _prev = m.state.compute.insert("oa".into(), granted_run("org"));
    let _prev = m.state.compute.insert("na".into(), granted_run("new"));
    let e = m
        .run_unproven(move_app("oa", "new", "alice", true, Value::Null))
        .unwrap_err();
    assert!(e.is_quota(), "{e}");
    let s = &m.state;
    assert_eq!(s.apps["oa"].org.as_str(), "org");
    assert_eq!((s.file_bytes("org"), s.file_bytes("new")), (100, 50));
    assert_eq!(s.orgs["new"].role("bob"), None, "nobody joined");

    m.state.quotas.max_concurrent_runs_per_org = 2;
    let _ok = m
        .run_unproven(move_app("oa", "new", "alice", true, Value::Null))
        .unwrap();
    assert_eq!(m.state.file_bytes("new"), 150);
}

/// A move judges the target's quotas as every other charge does: its own
/// override where the operator set one (`SetOrgQuotas`), else the cluster's.
/// An entry logged before the target's override counted (`org_quotas`
/// absent) replays judging the cluster's limits, as it did then.
#[test]
fn a_move_is_judged_by_the_targets_own_quota() {
    let setup = |org_quotas: bool| {
        let (mut m, _alice, _bob) = moving();
        m.state.quotas.max_file_bytes_per_org = 150;
        m.state.quotas.max_concurrent_runs_per_org = 1;
        m.state.set_app_file_bytes("oa", 100);
        let _prev = m.state.compute.insert("oa".into(), granted_run("org"));
        let mut cmd = move_app("oa", "new", "alice", true, Value::Null);
        if let SystemCmd::MoveApp { org_quotas: f, .. } = &mut cmd {
            *f = org_quotas;
        }
        (m, cmd)
    };

    // The target may hold more than the cluster's default: the move fits.
    let (mut m, cmd) = setup(true);
    let _prev = m.state.org_quotas.insert(
        "new".into(),
        super::OrgQuotas {
            max_concurrent_runs: Some(4),
            compute_fuel: None,
            max_file_bytes: Some(1_000),
        },
    );
    m.state.set_app_file_bytes("na", 0);
    let _prev = m.state.compute.insert("na".into(), granted_run("new"));
    let _ok = m.run_unproven(cmd).unwrap();
    assert_eq!(m.state.apps["oa"].org.as_str(), "new");
    assert_eq!(m.state.file_bytes("new"), 100);

    // The target may hold less: the move that fits the default is refused.
    let (mut m, cmd) = setup(true);
    let _prev = m.state.compute.remove("oa");
    let _prev = m.state.org_quotas.insert(
        "new".into(),
        super::OrgQuotas {
            max_concurrent_runs: None,
            compute_fuel: None,
            max_file_bytes: Some(50),
        },
    );
    let e = m.run_unproven(cmd).unwrap_err();
    assert!(e.is_quota(), "{e}");
    assert_eq!(m.state.apps["oa"].org.as_str(), "org");

    // An entry from before the flag replays judging the cluster's limits.
    let (mut m, cmd) = setup(false);
    let _prev = m.state.compute.remove("oa");
    let _prev = m.state.org_quotas.insert(
        "new".into(),
        super::OrgQuotas {
            max_concurrent_runs: None,
            compute_fuel: None,
            max_file_bytes: Some(50),
        },
    );
    let _ok = m.run_unproven(cmd).unwrap();
    assert_eq!(m.state.apps["oa"].org.as_str(), "new");
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
        m.state.apps.get_mut("oa").unwrap().access = crate::types::Access::team(team.into());
        let e = m
            .run_unproven(move_app("oa", "new", "alice", true, Value::Null))
            .unwrap_err();
        assert_eq!(e, super::MOVE_NEEDS_APP_TEAM, "{team}");
    }
    m.state.apps.get_mut("oa").unwrap().access = crate::types::Access::org();
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
    let team = out["grants"][0]["team"].as_str().unwrap().to_owned();
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
        a.org = "org".into();
        a.access = crate::types::Access::team("pt".into());
        a
    });
    let out = m
        .run_unproven(move_app("ob", "new", "alice", true, Value::Null))
        .unwrap();
    assert_eq!(out["created"], false, "{out}");
    assert_eq!(out["grants"][0]["team"], team, "{out}");

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
    assert_eq!(out["grants"], json!([{ "kind": "org" }]), "{out}");
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
    let team = out["grants"][0]["team"].as_str().unwrap().to_owned();
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

/// An instance only named people reach (a DM) of a package whose app team in the
/// target is closed is reached there only by those in that team: a move
/// with its people gives each one who reaches it now (bob, a member, and
/// carol, a guest) a plain row on that team, and they reach it after. A
/// move an earlier release logged gives none; an open team needs none.
#[test]
fn a_moved_dm_brings_its_people_into_the_targets_closed_app_team() {
    let dm: AppRecord = serde_json::from_value(json!({
        "appId": "dm", "name": "dm", "group": 6, "bundle": "b1", "owner": "bob",
        "org": "org", "grants": [{ "kind": "account", "account": "bob" }, { "kind": "account", "account": "carol" }],
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
    assert_eq!(m.state.apps["app"].org.as_str(), "org");
    assert_eq!(m.state.apps["app"].access, before);
    assert_eq!(m.state.file_bytes("org"), 7);
    assert_eq!(m.state.file_bytes("account:alice"), 0);

    // Bob's own app, into an organization where he is only a member.
    let _prev = m.state.apps.insert("bp".into(), {
        let mut a = m.state.apps["app"].clone();
        a.app_id = "bp".into();
        a.org = p("bob");
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
    assert!(m.state.may_upgrade(&m.state.apps["oa"], "bob", false));
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
        assert!(!m.state.may_upgrade(app, "bob", false), "{demoted:?}");
        assert!(m.run(set_access("oa", "bob")).is_err(), "{demoted:?}");
        assert!(m.run(set_app_metadata("oa", "bob")).is_err(), "{demoted:?}");
        assert!(m.run(delete_app("oa", "bob")).is_err(), "{demoted:?}");
        let transfer = transfer_app(&mut bob, "oa", "bob", "carol");
        assert!(m.run(transfer).is_err(), "{demoted:?}");
    }
    let app = &m.state.apps["oa"];
    assert_eq!(
        (app.owner.as_str(), &app.access),
        ("bob", &Access::default())
    );
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
    m.state.apps.get_mut("oa").unwrap().access = Access::team("t".into());
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
        assert!(!m.state.may_upgrade(app, "bob", false));
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
        .may_upgrade_as_logged(&m.state.apps["oa"], "bob", OwnerRule::InOrg, false));
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
    assert!(!m.state.may_upgrade(&m.state.apps["oa"], "bob", false));
    assert!(m
        .state
        .may_upgrade_as_logged(&m.state.apps["oa"], "bob", OwnerRule::Ownership, false));
    let refused = m
        .run(old(transfer_app(&mut alice, "oa", "alice", "carol")))
        .unwrap_err();
    assert_eq!(refused, "only the owner may transfer");
    let _ok = m.run(old(set_app_metadata("oa", "bob"))).unwrap();
    let _ok = m.run(old(set_access("oa", "bob"))).unwrap();
    assert_eq!(
        m.state.apps["oa"].access,
        Access::new(vec![crate::types::Grant::Anyone])
    );
    let _ok = m
        .run(old(transfer_app(&mut bob, "oa", "bob", "carol")))
        .unwrap();
    assert_eq!(m.state.apps["oa"].owner, "carol");
    let _gone = m.state.orgs.get_mut("org").unwrap().members.remove("carol");
    let _ok = m.run(old(delete_app("oa", "carol"))).unwrap();
    assert!(!m.state.apps.contains_key("oa"));
}

/// From the `RequireCurrentRules` mark on, an entry's flags naming an
/// earlier rule are read as set, whoever proposed it: an owner who left
/// the organization does not change, transfer or delete its app by
/// leaving `owner_in_org` out. An entry before the mark, replayed after a
/// restart, still applies as it did.
#[test]
fn app_commands_after_the_current_rules_mark_apply_under_them() {
    let (mut m, _alice, mut bob) = org_app_of_bob();
    let _gone = m.state.orgs.get_mut("org").unwrap().members.remove("bob");
    let old = |cmd: SystemCmd| {
        let mut v = serde_json::to_value(cmd).unwrap();
        let _flag = v.as_object_mut().unwrap().remove("owner_in_org");
        let _flag = v.as_object_mut().unwrap().remove("owner_in_team");
        serde_json::from_value::<SystemCmd>(v).unwrap()
    };
    m.index = 10;
    let _ok = m.run(SystemCmd::RequireCurrentRules).unwrap();
    assert_eq!(m.state.current_rules_from, Some(10));
    assert!(m.run(SystemCmd::RequireCurrentRules).is_err(), "once");
    m.index = 11;
    assert!(m.run(old(set_app_metadata("oa", "bob"))).is_err());
    assert!(m.run(old(set_access("oa", "bob"))).is_err());
    assert!(m
        .run(old(transfer_app(&mut bob, "oa", "bob", "carol")))
        .is_err());
    assert!(m.run(old(delete_app("oa", "bob"))).is_err());
    assert_eq!(m.state.apps["oa"].owner, "bob");
    assert_eq!(m.state.apps["oa"].access, Access::default());

    // A restart from a snapshot before the mark replays what came before it.
    m.index = 4;
    let _ok = m.run(old(set_access("oa", "bob"))).unwrap();
    assert_eq!(
        m.state.apps["oa"].access,
        Access::new(vec![crate::types::Grant::Anyone])
    );
    // State from before reads without the mark and writes as it did.
    let before = serde_json::to_value(SystemState::default()).unwrap();
    assert!(before.get("currentRulesFrom").is_none());
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

    // An account's personal organization's releases stay its own.
    let own = SystemCmd::SetReleaseApproval {
        scope: ReleaseScope::Org { org: p("erin") },
        version: "2.4.0".into(),
        approved: true,
        actor: "erin".into(),
        consent: serde_json::Value::Null,
    };
    let _ok = m.run_unproven(own).unwrap();
    assert!(m.state.orgs[&p("erin")].approved_releases.contains("2.4.0"));

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
    use super::{PackageVersion, OPERATOR};
    const T: u64 = 1_000;
    let mut m = SystemMachine::default();
    let _ok = m.run_unproven(genesis(all(Authority::Owner))).unwrap();
    for who in ["alice", "bob"] {
        let _ok = m.run_unproven(account(who)).unwrap();
        if let Some(o) = m.state.orgs.get_mut(&p(who)) {
            let _new = o.approved_releases.insert("2.0.0".into());
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
                "org": org, "grants": [{ "kind": "org" }], "replicas": [1], "createdAt": at
            }))
            .unwrap(),
        ),
        unit_offer: None,
        offers_only: false,
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
        in_deployment: false,
        owner_approves: false,
        class: None,
        auto: false,
        members_stay: false,
        restore_point: None,
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
    let refused = |r: Result<serde_json::Value, super::Rejection>| {
        let e = r.unwrap_err();
        assert!(e.is_deadline() && e.message().contains("2.1.0"), "{e}");
    };
    refused(m.run_unproven(app("late", "a", "alice", T)));
    refused(m.run_unproven(upgrade("old", "alice", T)));
    refused(m.run_unproven(install("alice", "1.1.0", T)));
    assert!(m.state.apps.contains_key("old"), "its apps stay");
    assert_eq!(m.state.stalled_for("a", T).unwrap().release, "2.1.0");
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
    assert!(m.state.stalled_for("a", T + 9).is_none());
    assert_eq!(m.state.stalled_for("b", T + 10).unwrap().release, "2.2.0");
    // Lifted by the operator: gone.
    let _ok = m
        .run_unproven(SystemCmd::SetReleaseDeadline {
            version: "2.2.0".into(),
            after: None,
            proof: serde_json::Value::Null,
        })
        .unwrap();
    assert!(m.state.stalled_for("b", T + 10).is_none());
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
                "org": "cust", "grants": [{ "kind": "org" }], "replicas": [1], "createdAt": 0
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
    assert_eq!(
        m.state.publisher_of(m.state.governing_release(&inst(&m))),
        Some(&"pub".to_owned())
    );
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
                "org": "cust", "grants": [{ "kind": "org" }], "replicas": [1], "createdAt": 10
            }))
            .unwrap(),
        ),
        unit_offer: None,
        offers_only: false,
    };
    let e = m.run_unproven(create("second", "b1")).unwrap_err();
    assert!(e.is_deadline(), "{e}");
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
    assert_eq!(
        m.state.publisher_of(m.state.governing_release(&inst(&m))),
        Some(&"cust".to_owned())
    );
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
            personal: false,
            app_team: None,
            capabilities: None,
        })
        .unwrap();
    assert_eq!(m.state.publisher_of_signer(&signer), None);
    let p1 = PackageRef {
        package: "p".into(),
        version: "1.0.0".into(),
        service: None,
        compute: false,
    };
    assert_eq!(m.state.publisher_of(Some(&p1)), None);
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
        m.state
            .upgrade_key_fits(&r, r.upgrades.as_ref().unwrap(), false),
        Ok(())
    );
    assert!(
        m.state
            .upgrade_key_fits(
                &record(100),
                &crate::types::UpgradeScope {
                    app: "rawapp".into(),
                    ..r.upgrades.clone().unwrap()
                },
                false
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
            in_deployment: false,
            owner_approves: false,
            class: None,
            auto: false,
            members_stay: false,
            restore_point: None,
        }
    };
    let e = m.run(upgrade("b2", 10, "cal_other")).unwrap_err();
    assert!(e.message().contains("not signed with the API key"), "{e}");
    let e = m.run(upgrade("raw", 10, secret)).unwrap_err();
    assert!(e.message().contains("only to signed releases"), "{e}");
    let e = m.run(upgrade("b2", 100, secret)).unwrap_err();
    assert!(e.message().contains("expired"), "{e}");
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
    assert!(e.message().contains("used already"), "{e}");

    // Its maker loses the right to upgrade: so does the key.
    if let Some(o) = m.state.orgs.get_mut("cust") {
        let _prev = o.capabilities.insert("dan".into(), 0);
    }
    assert!(!m.state.may_upgrade(&m.state.apps["inst"], "dan", false));
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
    assert!(e.message().contains("signer it names"), "{e}");
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
    assert!(e.message().contains("revoked"), "{e}");
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
    let may = |m: &SystemMachine, who: &str| m.state.may_upgrade(&inst(m), who, false);

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
        in_deployment: false,
        owner_approves: false,
        class: None,
        auto: false,
        members_stay: false,
        restore_point: None,
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

/// An upgrade of `app` to `bundle` of `release` (`package@version`) by `actor`.
fn upgrade_to(app: &str, actor: &str, bundle: &str, release: Option<(&str, &str)>) -> SystemCmd {
    SystemCmd::UpgradeApp {
        compute_bundle: None,
        app_id: app.into(),
        bundle: bundle.into(),
        by_operator: false,
        actor: actor.into(),
        at: 10,
        run: None,
        consent: serde_json::Value::Null,
        release: release.map(|(package, version)| PackageRef {
            package: package.into(),
            version: version.into(),
            service: None,
            compute: false,
        }),
        owner_in_org: true,
        owner_in_team: true,
        in_deployment: false,
        owner_approves: false,
        class: None,
        auto: false,
        members_stay: false,
        restore_point: None,
    }
}

/// The customer's instance `app` of `cust`, running `bundle` as
/// `release`, created at 0.
fn instance_of(m: &mut SystemMachine, app: &str, bundle: &str, release: Value) {
    let _prev = m.state.apps.insert(
        app.into(),
        serde_json::from_value(json!({
            "appId": app, "name": app, "group": 3, "bundle": bundle, "owner": "cora",
            "org": "cust", "grants": [{ "kind": "org" }], "replicas": [1], "createdAt": 0,
            "release": release
        }))
        .unwrap(),
    );
}

/// A release that ships the WASM of an earlier one of its package is an
/// upgrade of its own: an instance running it is upgraded as that
/// release says, not as the first release with that WASM did. An
/// `owner` release is its owners' even when a `publisher` one shipped
/// the same WASM first. Entries logged before `AttributeByRelease`
/// replay judging the bundle's first release, as they applied.
#[test]
fn an_instance_is_upgraded_as_the_release_it_runs_says() {
    use crate::types::caps::MANAGE_APPLICATION;
    let (key, signer) = publisher_key(10);
    let mut m = publishing(&signer, "publisher");
    if let Some(o) = m.state.orgs.get_mut("pub") {
        let _prev = o.capabilities.insert("bob".into(), MANAGE_APPLICATION);
    }
    let _ok = m
        .run_unproven(bind(&key, &signer, ("pub", "pub"), 0, "pam"))
        .unwrap();
    let _ok = m.run_unproven(SystemCmd::AttributeByRelease).unwrap();
    assert!(
        m.run_unproven(SystemCmd::AttributeByRelease).is_err(),
        "once"
    );
    // 2.0.0 gives upgrades to owners and ships 1.0.0's WASM.
    let _ok = m
        .run_unproven(SystemCmd::InstallPackage {
            package: "p".into(),
            signer_id: signer.clone(),
            release: serde_json::from_value(json!({
                "version": "2.0.0", "mpk": "20", "mpkSize": 1, "source": "upload",
                "services": { "": "b1" }, "installedBy": "pam", "installedAt": 0,
                "upgrades": "owner"
            }))
            .unwrap(),
        })
        .unwrap();
    instance_of(
        &mut m,
        "own",
        "b1",
        json!({ "package": "p", "version": "2.0.0" }),
    );
    let may = |m: &SystemMachine, app: &str, who: &str| {
        m.state.may_upgrade(&m.state.apps[app], who, false)
    };
    assert!(may(&m, "own", "cora"), "the owner upgrades it");
    assert!(
        !may(&m, "own", "pam") && !may(&m, "own", "bob"),
        "the publisher does not"
    );
    assert!(!m.state.upgrades_unbound(&m.state.apps["own"]));
    // An instance that names no release is judged by its bundle's.
    assert!(may(&m, "inst", "bob") && !may(&m, "inst", "cora"));

    let e = m
        .run_unproven(upgrade_to("own", "pam", "b1", Some(("p", "2.0.0"))))
        .unwrap_err();
    assert!(e.message().contains("needs its owner"), "{e}");
    let e = m
        .run_unproven(upgrade_to("own", "cora", "b2", Some(("p", "1.1.0"))))
        .unwrap_err();
    assert!(
        e.message().contains("not moved"),
        "never to a publisher release: {e}"
    );

    // Replayed before the mark, the bundle's first release judged it.
    let mut old = SystemMachine {
        state: m.state.clone(),
        ..Default::default()
    };
    old.state.attributes_by_release = false;
    assert!(may(&old, "own", "pam") && !may(&old, "own", "cora"));
}

/// A WASM belongs to one signer: a release of another signer's package
/// that ships it is refused, so that signer never gains
/// the upgrades or the release approvals of the instances that run it.
/// Where a cluster holds such a release from before, an instance is
/// judged by the release it runs, and a release installed after an
/// instance took a raw upload of the same WASM does not take it over.
#[test]
fn a_wasm_of_one_package_gives_another_no_authority() {
    use crate::types::caps::MANAGE_APPLICATION;
    let (key, signer) = publisher_key(11);
    let (evil_key, evil_signer) = publisher_key(12);
    let mut m = publishing(&signer, "owner");
    let _ok = m.run_unproven(account("eve")).unwrap();
    let mut o = org(
        &[("eve", "owner")],
        &[("eve", MANAGE_APPLICATION)],
        &["0.1.0"],
    );
    o.org_id = "evil".into();
    let _prev = m.state.orgs.insert("evil".into(), o);
    let _ok = m
        .run_unproven(bind(&key, &signer, ("pub", "pub"), 0, "pam"))
        .unwrap();
    let _ok = m
        .run_unproven(bind(&evil_key, &evil_signer, ("evil", "evil"), 0, "eve"))
        .unwrap();
    let _ok = m.run_unproven(SystemCmd::AttributeByRelease).unwrap();
    instance_of(
        &mut m,
        "victim",
        "b1",
        json!({ "package": "p", "version": "1.0.0" }),
    );
    let q = |version: &str, wasm: &str, at: u64| SystemCmd::InstallPackage {
        package: "q".into(),
        signer_id: evil_signer.clone(),
        release: serde_json::from_value(json!({
            "version": version, "mpk": version, "mpkSize": 1, "source": "upload",
            "services": { "": wasm }, "installedBy": "eve", "installedAt": at,
            "upgrades": "publisher"
        }))
        .unwrap(),
    };
    let e = m.run_unproven(q("1.0.0", "b1", 0)).unwrap_err();
    assert_eq!(e, SHIPS_IN_OTHER_PACKAGE);
    assert!(!m.state.packages.contains_key("q"));

    // A cluster that installed one before: the bundle is recorded under
    // `q`, and the instance is still judged by the release it runs.
    let mut legacy = SystemMachine {
        state: m.state.clone(),
        ..Default::default()
    };
    legacy.state.attributes_by_release = false;
    let _ok = legacy.run_unproven(q("1.0.0", "b1", 0)).unwrap();
    if let Some(b) = legacy.state.bundles.get_mut("b1") {
        b.package = Some(PackageRef {
            package: "q".into(),
            version: "1.0.0".into(),
            service: None,
            compute: false,
        });
    }
    let victim = |m: &SystemMachine| m.state.apps["victim"].clone();
    assert!(
        legacy.state.may_upgrade(&victim(&legacy), "eve", false),
        "as replayed"
    );
    legacy.state.attributes_by_release = true;
    assert!(!legacy.state.may_upgrade(&victim(&legacy), "eve", false));
    assert!(legacy.state.may_upgrade(&victim(&legacy), "cora", false));
    assert_eq!(
        legacy.state.approved_releases(&victim(&legacy)),
        m.state.orgs.get("pub").map(|o| &o.approved_releases),
        "the victim's publisher approves its releases"
    );

    // A raw upload's instance: a release installed later that ships the
    // same WASM takes nothing from its owner.
    let _ok = m.run_unproven(q("1.0.0", "raw", 5)).unwrap();
    let rawapp = m.state.apps["rawapp"].clone();
    assert_eq!(
        m.state.bundles["raw"]
            .package
            .as_ref()
            .map(|r| r.package.as_str()),
        Some("q")
    );
    assert!(!m.state.may_upgrade(&rawapp, "eve", false));
    assert!(m.state.may_upgrade(&rawapp, "cora", false));
    assert_eq!(
        m.state.approved_releases(&rawapp),
        m.state.orgs.get("cust").map(|o| &o.approved_releases)
    );

    // The same signer may ship the WASM in another package of its own:
    // each instance is governed by the release it runs.
    let _ok = m
        .run_unproven(SystemCmd::InstallPackage {
            package: "p2".into(),
            signer_id: signer.clone(),
            release: serde_json::from_value(json!({
                "version": "1.0.0", "mpk": "p2-1", "mpkSize": 1, "source": "upload",
                "services": { "": "b1" }, "installedBy": "pam", "installedAt": 6,
                "upgrades": "publisher"
            }))
            .unwrap(),
        })
        .unwrap();
    instance_of(
        &mut m,
        "twin",
        "b1",
        json!({ "package": "p2", "version": "1.0.0" }),
    );
    let twin = m.state.apps["twin"].clone();
    assert!(m.state.may_upgrade(&victim(&m), "cora", false));
    assert!(!m.state.may_upgrade(&victim(&m), "pam", false));
    assert!(
        m.state.may_upgrade(&twin, "pam", false),
        "p2 says publisher"
    );
    assert!(!m.state.may_upgrade(&twin, "cora", false));
}

/// From `Feature::OwnerApprovedUpgrades` on, only an owner or admin of an app's
/// organization approves its upgrades, whatever its package declares: not the app's
/// creator as a member, a team's lead or a member holding `MANAGE_APPLICATION`, nor
/// the publisher's organization for an instance elsewhere; a `publisher` package's
/// instance is its organization's to upgrade, its key bound or not. Judged at apply
/// for an entry that says so (`owner_approves`) or once the logged cluster version
/// brings the rule; an entry logged before replays as it applied. Runs and upgrade
/// keys follow the same rule.
#[test]
fn only_an_organizations_owner_and_admins_approve_its_upgrades() {
    use super::cluster_version::Feature;
    use super::NOT_AN_UPGRADE_APPROVER;
    use crate::types::caps::MANAGE_APPLICATION;
    let (key, signer) = publisher_key(14);
    let mut m = publishing(&signer, "owner");
    for who in ["eve", "lee", "mia"] {
        let _ok = m.run_unproven(account(who)).unwrap();
    }
    if let Some(o) = m.state.orgs.get_mut("cust") {
        for (who, role) in [
            ("eve", crate::types::OrgRole::Admin),
            ("lee", crate::types::OrgRole::Member),
            ("mia", crate::types::OrgRole::Member),
        ] {
            let _prev = o.members.insert(who.into(), role);
        }
        let _prev = o.capabilities.insert("mia".into(), MANAGE_APPLICATION);
        let _prev = o.teams.insert(
            "t".into(),
            serde_json::from_value(json!({
                "teamId": "t", "name": "T", "join": "invite",
                "members": { "lee": true, "dan": false }
            }))
            .unwrap(),
        );
    }
    for (id, grants) in [
        ("mine", json!([{ "kind": "org" }])),
        ("onteam", json!([{ "kind": "team", "team": "t" }])),
    ] {
        let _prev = m.state.apps.insert(
            id.into(),
            serde_json::from_value(json!({
                "appId": id, "name": id, "group": 4, "bundle": "b1", "owner": "dan",
                "org": "cust", "grants": grants, "replicas": [1], "createdAt": 0,
                "release": { "package": "p", "version": "1.0.0" }
            }))
            .unwrap(),
        );
    }
    let may = |m: &SystemMachine, app: &str, who: &str, rule: bool| {
        m.state.may_upgrade(&m.state.apps[app], who, rule)
    };
    // As before: the creator while a member, the team's lead, a holder of the capability.
    assert!(may(&m, "mine", "dan", false));
    assert!(may(&m, "onteam", "lee", false));
    assert!(may(&m, "mine", "mia", false));
    // Under the rule: the owner and admins only.
    for app in ["mine", "onteam"] {
        for (who, expected) in [
            ("cora", true),
            ("eve", true),
            ("dan", false),
            ("lee", false),
            ("mia", false),
            ("pam", false),
        ] {
            assert_eq!(may(&m, app, who, true), expected, "{app} {who}");
        }
    }
    let flagged = |app: &str, who: &str| {
        let mut cmd = upgrade_to(app, who, "b2", Some(("p", "1.1.0")));
        if let SystemCmd::UpgradeApp { owner_approves, .. } = &mut cmd {
            *owner_approves = true;
        }
        cmd
    };
    assert_eq!(
        flagged("mine", "dan").features(),
        [Feature::OwnerApprovedUpgrades]
    );
    assert!(upgrade_to("mine", "dan", "b2", None).features().is_empty());
    for (app, who) in [("mine", "dan"), ("onteam", "lee"), ("mine", "mia")] {
        assert_eq!(
            m.run_unproven(flagged(app, who)),
            Err(NOT_AN_UPGRADE_APPROVER.into()),
            "{app} {who}"
        );
    }
    // An entry logged before the rule replays as it applied.
    let mut old = SystemMachine {
        state: m.state.clone(),
        ..Default::default()
    };
    let _ok = old
        .run_unproven(upgrade_to("mine", "dan", "b2", Some(("p", "1.1.0"))))
        .unwrap();
    assert_eq!(old.state.apps["mine"].bundle, "b2");
    // Once the logged cluster version brings it, an entry without the flag is judged by it.
    let mut new = SystemMachine {
        state: m.state.clone(),
        ..Default::default()
    };
    new.state.cluster_version = Some(Feature::OwnerApprovedUpgrades.since().into());
    assert_eq!(
        new.run_unproven(upgrade_to("mine", "dan", "b2", Some(("p", "1.1.0")))),
        Err(NOT_AN_UPGRADE_APPROVER.into())
    );
    let _ok = m.run_unproven(flagged("onteam", "eve")).unwrap();
    let _ok = m.run_unproven(flagged("mine", "cora")).unwrap();
    assert_eq!(m.state.apps["mine"].bundle, "b2");

    // A run: only the organization's owner or an admin starts one.
    let run = |by: &str, id: &str, flag: bool| -> SystemCmd {
        serde_json::from_value(json!({ "op": "startUpgradeRun", "org": "cust",
            "run": { "runId": id, "package": "p", "version": "1.1.0", "by": by,
                "startedAt": 1, "apps": [], "done": [], "failed": {}, "finished": false },
            "owner_approves": flag }))
        .unwrap()
    };
    assert_eq!(
        run("mia", "r1", true).features(),
        [Feature::OwnerApprovedUpgrades]
    );
    assert_eq!(
        m.run_unproven(run("mia", "r1", true)),
        Err(NOT_AN_UPGRADE_APPROVER.into())
    );
    let _ok = m.run_unproven(run("mia", "r0", false)).unwrap();
    let _ok = m.run_unproven(run("eve", "r2", true)).unwrap();

    // An upgrade key: made only for an account the rule lets upgrade the app.
    let record = |account: &str| super::ApiKeyRecord {
        key_id: "k1".into(),
        account: account.into(),
        label: "ci".into(),
        apps: None,
        methods: None,
        read_only: false,
        allow_confirm: false,
        expires_at: 100,
        upgrades: Some(crate::types::UpgradeScope {
            app: "inst".into(),
            package: "p".into(),
            signer: signer.clone(),
            proof_key: String::new(),
        }),
    };
    let fits = |account: &str, rule: bool| {
        let r = record(account);
        m.state
            .upgrade_key_fits(&r, r.upgrades.as_ref().unwrap(), rule)
    };
    assert_eq!(fits("mia", false), Ok(()));
    assert_eq!(fits("mia", true), Err(NOT_AN_UPGRADE_APPROVER));
    assert_eq!(fits("eve", true), Ok(()));

    // A package its publisher upgrades: the customer's owner and admins approve, the
    // publisher's admins do not, bound or not; a publisher's run covers only the
    // organizations its starter is an owner or admin of.
    let mut sealed = publishing(&signer, "publisher");
    let _ok = sealed.run_unproven(account("eve")).unwrap();
    if let Some(o) = sealed.state.orgs.get_mut("cust") {
        let _prev = o.members.insert("eve".into(), crate::types::OrgRole::Admin);
    }
    if let Some(o) = sealed.state.orgs.get_mut("pub") {
        let _prev = o.members.insert("bob".into(), crate::types::OrgRole::Admin);
    }
    let inst = |m: &SystemMachine, who: &str, rule: bool| {
        m.state.may_upgrade(&m.state.apps["inst"], who, rule)
    };
    assert!(!inst(&sealed, "cora", false), "unbound: nobody, as before");
    assert!(inst(&sealed, "cora", true) && inst(&sealed, "eve", true));
    let _ok = sealed
        .run_unproven(bind(&key, &signer, ("pub", "pub"), 0, "pam"))
        .unwrap();
    assert!(inst(&sealed, "bob", false), "as before: the publisher's");
    for (who, expected) in [
        ("cora", true),
        ("eve", true),
        ("pam", false),
        ("bob", false),
    ] {
        assert_eq!(inst(&sealed, who, true), expected, "{who}");
    }
    let _prev = sealed.state.apps.insert(
        "own".into(),
        serde_json::from_value(json!({
            "appId": "own", "name": "own", "group": 5, "bundle": "b1", "owner": "pam",
            "org": "pub", "grants": [{ "kind": "org" }], "replicas": [1], "createdAt": 0,
            "release": { "package": "p", "version": "1.0.0" }
        }))
        .unwrap(),
    );
    let _ok = sealed.run_unproven(SystemCmd::AttributeByRelease).unwrap();
    let mut before = sealed.state.published_run_apps("p", "bob", true, false);
    before.sort();
    assert_eq!(before, ["inst", "own"], "as before: every instance");
    assert_eq!(
        sealed.state.published_run_apps("p", "bob", true, true),
        ["own"]
    );
    let sealed_up = |who: &str| {
        let mut cmd = upgrade_to("inst", who, "b2", Some(("p", "1.1.0")));
        if let SystemCmd::UpgradeApp { owner_approves, .. } = &mut cmd {
            *owner_approves = true;
        }
        cmd
    };
    assert_eq!(
        sealed.run_unproven(sealed_up("bob")),
        Err(NOT_AN_UPGRADE_APPROVER.into())
    );
    let _ok = sealed.run_unproven(sealed_up("eve")).unwrap();
    assert_eq!(sealed.state.apps["inst"].bundle, "b2");
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

/// Where deployments are granted, an instance from no deployment is a deployer's, or of a
/// release the operator installed; judged at apply when the entry says so or the logged
/// cluster version brings the rule, so an entry from before replays as it applied.
#[test]
fn on_a_granted_cluster_an_instance_outside_every_deployment_is_a_deployers() {
    use super::{cluster_version::Feature, Deployments, PackageRef, NOT_A_DEPLOYER, OPERATOR};
    let outside = |id: &str, owner: &str, version: &str, offers_only: bool| {
        let mut cmd = instance(id, owner, &format!("b-{version}"), None);
        if let SystemCmd::CreateApp {
            app,
            offers_only: flag,
            ..
        } = &mut cmd
        {
            app.release = Some(PackageRef {
                package: "p".into(),
                version: version.into(),
                service: None,
                compute: false,
            });
            *flag = offers_only;
        }
        cmd
    };
    let mut m = deploying(Deployments::Granted);
    let _ok = m
        .run_unproven(grant(account_deployer("alice"), None, OPERATOR))
        .unwrap();
    let _ok = m.run_unproven(install_at("alice", "1.0.0", 5)).unwrap();
    let _ok = m.run_unproven(install_at(OPERATOR, "0.9.0", 5)).unwrap();

    assert_eq!(
        m.run_unproven(outside("b1", "bob", "1.0.0", true))
            .unwrap_err(),
        NOT_A_DEPLOYER
    );
    let _ok = m
        .run_unproven(outside("a1", "alice", "1.0.0", true))
        .unwrap();
    let _ok = m.run_unproven(outside("b2", "bob", "0.9.0", true)).unwrap();
    let _ok = m
        .run_unproven(outside("b3", "bob", "1.0.0", false))
        .unwrap();

    m.state.cluster_version = Some(Feature::OffersOnly.since().into());
    assert_eq!(
        m.run_unproven(outside("b4", "bob", "1.0.0", false))
            .unwrap_err(),
        NOT_A_DEPLOYER,
        "bob shares org with alice, the installer, and still needs an offer"
    );
    let _ok = m
        .run_unproven(SystemCmd::RevokeDeployer {
            deployer: account_deployer("alice"),
            at: 6,
            actor: OPERATOR.into(),
            proof: None,
            consent: None,
        })
        .unwrap();
    assert_eq!(
        m.run_unproven(outside("a2", "alice", "1.0.0", false))
            .unwrap_err(),
        NOT_A_DEPLOYER,
        "a revoked deployer, though it installed the release"
    );

    let mut open = deploying(Deployments::Open);
    let _ok = open.run_unproven(install_at("alice", "1.0.0", 5)).unwrap();
    let _ok = open
        .run_unproven(outside("b1", "bob", "1.0.0", true))
        .unwrap();
}

/// Where deployments are granted, an instance from no deployment moves into another
/// package only as its upgrader could create one there, from the logged version that
/// brings the rule; within its package, and before that version, it moves as it did.
#[test]
fn on_a_granted_cluster_an_instance_outside_every_deployment_keeps_out_of_a_deployers_package() {
    use super::{cluster_version::Feature, Deployments, NOT_A_DEPLOYER, OPERATOR};
    let mut m = deploying(Deployments::Granted);
    let _ok = m
        .run_unproven(grant(account_deployer("alice"), None, OPERATOR))
        .unwrap();
    let _ok = m.run_unproven(install_at("alice", "1.0.0", 5)).unwrap();
    let _ok = m.run_unproven(install_at("alice", "1.1.0", 5)).unwrap();
    let catalog = SystemCmd::InstallPackage {
        package: "q".into(),
        signer_id: "did:key:q".into(),
        release: serde_json::from_value(json!({
            "version": "1.0.0", "mpk": "q", "mpkSize": 1, "source": "upload",
            "services": { "": "b-q" }, "installedBy": OPERATOR, "installedAt": 5
        }))
        .unwrap(),
    };
    let _ok = m.run_unproven(catalog).unwrap();
    let create =
        |m: &mut SystemMachine, id: &str, owner: &str, bundle: &str, release: (&str, &str)| {
            let mut cmd = instance(id, owner, bundle, None);
            if let SystemCmd::CreateApp { app, .. } = &mut cmd {
                app.release = Some(super::PackageRef {
                    package: release.0.into(),
                    version: release.1.into(),
                    service: None,
                    compute: false,
                });
            }
            m.run_unproven(cmd).unwrap();
        };
    create(&mut m, "early", "bob", "b-q", ("q", "1.0.0"));
    create(&mut m, "late", "bob", "b-q", ("q", "1.0.0"));
    create(&mut m, "own", "alice", "b-1.0.0", ("p", "1.0.0"));
    create(&mut m, "alice-q", "alice", "b-q", ("q", "1.0.0"));

    let _ok = m
        .run_unproven(upgrade_to("early", "bob", "b-1.0.0", Some(("p", "1.0.0"))))
        .unwrap();

    m.state.cluster_version = Some(Feature::OffersOnly.since().into());
    assert_eq!(
        m.run_unproven(upgrade_to("late", "bob", "b-1.0.0", Some(("p", "1.0.0"))))
            .unwrap_err(),
        NOT_A_DEPLOYER
    );
    let _ok = m
        .run_unproven(upgrade_to("early", "bob", "b-1.1.0", Some(("p", "1.1.0"))))
        .unwrap();
    let _ok = m
        .run_unproven(upgrade_to("own", "alice", "b-1.1.0", Some(("p", "1.1.0"))))
        .unwrap();

    // Revoked, alice enters her own package no more; within one she still moves.
    let _ok = m
        .run_unproven(SystemCmd::RevokeDeployer {
            deployer: account_deployer("alice"),
            at: 6,
            actor: OPERATOR.into(),
            proof: None,
            consent: None,
        })
        .unwrap();
    assert_eq!(
        m.run_unproven(upgrade_to(
            "alice-q",
            "alice",
            "b-1.1.0",
            Some(("p", "1.1.0"))
        ))
        .unwrap_err(),
        NOT_A_DEPLOYER
    );
    let _ok = m
        .run_unproven(upgrade_to("own", "alice", "b-1.0.0", Some(("p", "1.0.0"))))
        .unwrap();
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
                "appId": id, "name": id, "group": 0, "bundle": bundle, "owner": owner, "org": p(owner),
                "replicas": [1], "createdAt": 10, "deployment": deployment,
            }))
            .unwrap(),
        ),
        unit_offer: None,
        offers_only: false,
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
    use super::{Deployer, Deployments, MAX_DEPLOYMENTS_PER_OWNER};
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
    assert!(e.is_quota(), "{e}");
}

/// `kv@0.1.0`, a member's release (bundle `kv-0.1.0`), installed by `by`.
fn install_member_release(by: &str) -> SystemCmd {
    SystemCmd::InstallPackage {
        package: "kv".into(),
        signer_id: "did:key:z".into(),
        release: serde_json::from_value(json!({
            "version": "0.1.0", "mpk": "kv", "mpkSize": 1, "source": "upload",
            "services": { "": "kv-0.1.0" }, "installedBy": by, "installedAt": 5
        }))
        .unwrap(),
    }
}

/// `owner`'s app `id` of `bundle`/`release` from `deployment`, its unit's `links`, as the
/// log carries it; `unit_offer` (absent when `None`) names the offer covering it.
fn unit_app(
    id: &str,
    owner: &str,
    (bundle, release): (&str, Value),
    deployment: Option<&str>,
    links: Value,
    unit_offer: Option<&str>,
) -> SystemCmd {
    let mut v = json!({ "op": "createApp", "app": {
        "appId": id, "name": id, "group": 0, "bundle": bundle, "owner": owner, "org": p(owner),
        "replicas": [1], "createdAt": 10, "deployment": deployment, "release": release,
        "links": links,
    }});
    if let Some(version) = unit_offer {
        v["unit_offer"] = json!({ "package": "p", "version": version });
    }
    serde_json::from_value(v).unwrap()
}

fn kv_member(id: &str, deployment: Option<&str>, links: Value, offer: Option<&str>) -> SystemCmd {
    let release = json!({ "package": "kv", "version": "0.1.0" });
    unit_app(id, "bob", ("kv-0.1.0", release), deployment, links, offer)
}

fn notes_instance(id: &str, deployment: Option<&str>, links: Value) -> SystemCmd {
    let release = json!({ "package": "p", "version": "1.0.0" });
    unit_app(id, "bob", ("b-1.0.0", release), deployment, links, None)
}

/// `DeleteApp` of `app` by `actor` under the whole-unit rule, naming `members`.
fn delete_unit(app: &str, actor: &str, members: &[&str]) -> SystemCmd {
    SystemCmd::DeleteApp {
        app_id: app.into(),
        actor: actor.into(),
        owner_in_org: true,
        owner_in_team: true,
        members: Some(members.iter().map(|m| (*m).to_owned()).collect()),
    }
}

/// A unit is one: a member of an instance from a deployment carries the deployment, which
/// offers only the instance's release (`unit_offer` names that offer, checked at apply as
/// an instance's own release is); deleting the instance deletes every member, the members'
/// own included, when the entry names exactly those, by a manager of each; a member is
/// never deleted alone while its instance stands. Both shapes are `Feature::WholeUnits`.
#[test]
fn a_units_members_carry_its_deployment_and_go_with_it() {
    use super::cluster_version::Feature;
    use super::{Deployer, Deployments, UNIT_MEMBER};
    let mut m = deploying(Deployments::Open);
    let _ok = m
        .run_unproven(create_deployment(
            "a",
            Deployer::Account("alice".into()),
            "alice",
        ))
        .unwrap();
    let _ok = m.run_unproven(install_at("alice", "1.0.0", 5)).unwrap();
    let _ok = m.run_unproven(install_member_release("alice")).unwrap();
    let _ok = m
        .run_unproven(offer_app("a", "notes", "1.0.0", "alice"))
        .unwrap();

    // The member's own release is not offered: refused without the offer that covers it,
    // with an offer the deployment does not make, or from another deployment.
    let e = m
        .run_unproven(kv_member("d2", Some("a"), json!({}), None))
        .unwrap_err();
    assert!(e.message().contains("does not offer"), "{e}");
    let e = m
        .run_unproven(kv_member("d2", Some("a"), json!({}), Some("9.9.9")))
        .unwrap_err();
    assert!(e.message().contains("does not offer"), "{e}");
    assert!(m
        .run_unproven(kv_member("d2", Some("zz"), json!({}), Some("1.0.0")))
        .is_err());
    // A unit two deep: d1 created d2, the instance created d1.
    let deep = kv_member("d2", Some("a"), json!({}), Some("1.0.0"));
    assert_eq!(deep.features(), vec![Feature::WholeUnits]);
    let _ok = m.run_unproven(deep).unwrap();
    let _ok = m
        .run_unproven(kv_member(
            "d1",
            Some("a"),
            json!({ "kv": "d2" }),
            Some("1.0.0"),
        ))
        .unwrap();
    let _ok = m
        .run_unproven(notes_instance("i1", Some("a"), json!({ "kv": "d1" })))
        .unwrap();
    for id in ["i1", "d1", "d2"] {
        assert_eq!(m.state.apps[id].deployment.as_deref(), Some("a"), "{id}");
    }
    let mut unit = m.state.unit_of("i1");
    unit.sort();
    assert_eq!(unit, ["d1", "d2"]);
    assert_eq!(m.state.unit_parent("d2"), Some("d1"));
    assert_eq!(m.state.unit_parent("i1"), None);

    // A member alone, at either depth, is refused.
    for member in ["d1", "d2"] {
        assert_eq!(
            m.run_unproven(delete_unit(member, "bob", &[])).unwrap_err(),
            UNIT_MEMBER
        );
    }
    // The entry names the whole unit, no more and no less.
    assert!(m.run_unproven(delete_unit("i1", "bob", &["d1"])).is_err());
    assert!(m
        .run_unproven(delete_unit("i1", "bob", &["d1", "d2", "i1"]))
        .is_err());
    // Only who may delete the instance and each member.
    assert!(m
        .run_unproven(delete_unit("i1", "carol", &["d1", "d2"]))
        .is_err());
    // A member handed to carol in her own organization is not bob's to delete.
    let d2 = m.state.apps.get_mut("d2").unwrap();
    (d2.owner, d2.org) = ("carol".into(), p("carol"));
    assert!(m
        .run_unproven(delete_unit("i1", "bob", &["d2", "d1"]))
        .is_err());
    let d2 = m.state.apps.get_mut("d2").unwrap();
    (d2.owner, d2.org) = ("bob".into(), p("bob"));
    assert_eq!(m.state.apps.len(), 3);

    let whole = delete_unit("i1", "bob", &["d2", "d1"]);
    assert_eq!(whole.features(), vec![Feature::WholeUnits]);
    let _ok = m.run_unproven(whole).unwrap();
    assert!(m.state.apps.is_empty(), "{:?}", m.state.apps.keys());
    // With its instances gone, the deployment withdraws its offer.
    let _ok = m.run_unproven(withdraw_app("a", "notes", "alice")).unwrap();
}

/// Entries an earlier release logged replay as it applied them: a member made without the
/// instance's deployment, and a `DeleteApp` without `members` that deletes a member alone
/// or an instance without its unit. The entries this release logs without the rule read
/// the same (`unit_offer` and `members` are left out when absent).
#[test]
fn unit_entries_logged_before_whole_units_replay_as_they_did() {
    use super::{Deployer, Deployments};
    let mut m = deploying(Deployments::Open);
    let _ok = m
        .run_unproven(create_deployment(
            "a",
            Deployer::Account("alice".into()),
            "alice",
        ))
        .unwrap();
    let _ok = m.run_unproven(install_at("alice", "1.0.0", 5)).unwrap();
    let _ok = m.run_unproven(install_member_release("alice")).unwrap();
    let _ok = m
        .run_unproven(offer_app("a", "notes", "1.0.0", "alice"))
        .unwrap();
    let old_member = kv_member("k1", None, json!({}), None);
    assert!(old_member.features().is_empty());
    let v = serde_json::to_value(&old_member).unwrap();
    assert!(v.get("unit_offer").is_none(), "{v}");
    let _ok = m.run_unproven(old_member).unwrap();
    let _ok = m
        .run_unproven(kv_member("k2", None, json!({}), None))
        .unwrap();
    let _ok = m
        .run_unproven(notes_instance(
            "i1",
            Some("a"),
            json!({ "a": "k1", "b": "k2" }),
        ))
        .unwrap();
    assert_eq!(m.state.apps["k1"].deployment, None);

    let old_delete = delete_app("k1", "bob");
    assert!(old_delete.features().is_empty());
    let v = serde_json::to_value(&old_delete).unwrap();
    assert!(v.get("members").is_none(), "{v}");
    let _ok = m.run_unproven(old_delete).unwrap();
    assert!(!m.state.apps.contains_key("k1"));
    assert_eq!(
        m.state.apps["i1"].links["a"], "k1",
        "the link stays as the record"
    );
    let _ok = m.run_unproven(delete_app("i1", "bob")).unwrap();
    assert!(!m.state.apps.contains_key("i1"));
    assert!(
        m.state.apps.contains_key("k2"),
        "an old delete keeps the members"
    );
    // An orphan, its instance gone, is deleted alone under the new rule too.
    let _ok = m.run_unproven(delete_unit("k2", "bob", &[])).unwrap();
    assert!(m.state.apps.is_empty());
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
        "appId": "a", "name": "n", "group": 2, "bundle": "b", "owner": "alice", "org": p("alice"),
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

// ----------------------------------------------------- deployment secrets

/// `install_at`, the release declaring secret `API_KEY` for `api.example.com`.
fn install_declaring(by: &str, version: &str) -> SystemCmd {
    SystemCmd::InstallPackage {
        package: "p".into(),
        signer_id: "did:key:z".into(),
        release: serde_json::from_value(json!({
            "version": version, "mpk": version, "mpkSize": 1, "source": "upload",
            "services": { "": format!("b-{version}") }, "installedBy": by, "installedAt": 5,
            "outbound": ["https://api.example.com/"],
            "secrets": { "API_KEY": { "host": "api.example.com", "paths": ["/"], "header": "x-api-key" } },
        }))
        .unwrap(),
    }
}

fn set_secret(deployment: &str, name: &str, value: &str, actor: &str) -> SystemCmd {
    SystemCmd::SetDeploymentSecret {
        deployment: deployment.into(),
        name: name.into(),
        value: crate::component::SecretValue(value.into()),
        actor: actor.into(),
        at: 20,
        consent: serde_json::Value::Null,
        used: false,
    }
}

/// [`set_secret`] judged by the releases the offered ones reference too (`used`).
fn set_used_secret(deployment: &str, name: &str, value: &str, actor: &str) -> SystemCmd {
    let SystemCmd::SetDeploymentSecret {
        deployment,
        name,
        value,
        actor,
        at,
        consent,
        ..
    } = set_secret(deployment, name, value, actor)
    else {
        unreachable!("set_secret makes a SetDeploymentSecret")
    };
    SystemCmd::SetDeploymentSecret {
        deployment,
        name,
        value,
        actor,
        at,
        consent,
        used: true,
    }
}

fn clear_secret(deployment: &str, name: &str, actor: &str) -> SystemCmd {
    SystemCmd::ClearDeploymentSecret {
        deployment: deployment.into(),
        name: name.into(),
        actor: actor.into(),
        consent: serde_json::Value::Null,
    }
}

/// A deployment's managers set a value of a secret a release it offers
/// declares, read by every instance created from it; nobody else, no other
/// deployment, no undeclared or malformed name and no empty or oversized value.
/// Clearing takes it back; deleting the deployment drops its values. A value
/// whose name no offer declares any more is kept, unused, until cleared.
#[test]
fn a_deployments_managers_set_secrets_its_offers_declare() {
    use super::{
        Deployer, Deployments, MAX_SECRETS_PER_DEPLOYMENT, NO_DEPLOYMENT_SECRET, UNDECLARED_SECRET,
    };
    let mut m = deploying(Deployments::Open);
    let _ok = m.run_unproven(install_declaring("alice", "1.0.0")).unwrap();
    let _ok = m.run_unproven(install_at("alice", "2.0.0", 5)).unwrap();
    let _ok = m
        .run_unproven(create_deployment(
            "a",
            Deployer::Account("alice".into()),
            "alice",
        ))
        .unwrap();
    let _ok = m
        .run_unproven(create_deployment("o", Deployer::Org("org".into()), "carol"))
        .unwrap();
    // Nothing offered yet: nothing declares it.
    assert_eq!(
        m.run_unproven(set_secret("a", "API_KEY", "k1", "alice"))
            .unwrap_err(),
        UNDECLARED_SECRET
    );
    let _ok = m
        .run_unproven(offer_app("a", "notes", "1.0.0", "alice"))
        .unwrap();
    let _ok = m
        .run_unproven(offer_app("o", "notes", "1.0.0", "carol"))
        .unwrap();

    // Refused: another account, a plain member, a missing deployment, an
    // undeclared or malformed name, an empty or oversized value.
    assert!(m
        .run_unproven(set_secret("a", "API_KEY", "k1", "bob"))
        .is_err());
    assert!(m
        .run_unproven(set_secret("o", "API_KEY", "k1", "bob"))
        .is_err());
    assert!(m
        .run_unproven(set_secret("zz", "API_KEY", "k1", "alice"))
        .is_err());
    assert_eq!(
        m.run_unproven(set_secret("a", "OTHER", "k1", "alice"))
            .unwrap_err(),
        UNDECLARED_SECRET
    );
    assert!(m
        .run_unproven(set_secret("a", "api_key", "k1", "alice"))
        .is_err());
    assert!(m
        .run_unproven(set_secret("a", "API_KEY", "", "alice"))
        .is_err());
    let long = "x".repeat(crate::component::MAX_SECRET_VALUE_BYTES + 1);
    assert!(m
        .run_unproven(set_secret("a", "API_KEY", &long, "alice"))
        .is_err());
    assert!(m.state.deployment_secrets.is_empty());

    // Set: answered by fingerprint, never the value; an org's by its manager.
    let out = m
        .run_unproven(set_secret("a", "API_KEY", "alices-key", "alice"))
        .unwrap();
    assert!(!out.to_string().contains("alices-key"), "{out}");
    assert_eq!(
        out["fingerprint"],
        json!(crate::component::SecretValue("alices-key".into()).fingerprint())
    );
    assert_eq!(out["setBy"], json!("account:alice"));
    let _ok = m
        .run_unproven(set_secret("o", "API_KEY", "orgs-key", "carol"))
        .unwrap();
    // Replaced in place, within the bound.
    let _ok = m
        .run_unproven(set_secret("a", "API_KEY", "alices-new-key", "alice"))
        .unwrap();
    assert_eq!(m.state.deployment_secrets["a"].len(), 1);
    assert_eq!(MAX_SECRETS_PER_DEPLOYMENT, calimero_bundle::MAX_SECRETS);
    let held = m.state.deployment_secrets.get_mut("a").unwrap();
    for i in 1..MAX_SECRETS_PER_DEPLOYMENT {
        let mut filler = held["API_KEY"].clone();
        filler.set_at = u64::try_from(i).unwrap();
        let _prev = held.insert(format!("FILLER_{i}"), filler);
    }
    // A name the offers declare, but the deployment is full: refused; the
    // one it holds is still replaced.
    let _ok = m
        .run_unproven(SystemCmd::InstallPackage {
            package: "p".into(),
            signer_id: "did:key:z".into(),
            release: serde_json::from_value(json!({
                "version": "3.0.0", "mpk": "3.0.0", "mpkSize": 1, "source": "upload",
                "services": { "": "b-3.0.0" }, "installedBy": "alice", "installedAt": 5,
                "outbound": ["https://api.example.com/"],
                "secrets": { "MORE": { "host": "api.example.com", "paths": ["/"], "header": "x-more" } },
            }))
            .unwrap(),
        })
        .unwrap();
    let _ok = m
        .run_unproven(offer_app("a", "more", "3.0.0", "alice"))
        .unwrap();
    let e = m
        .run_unproven(set_secret("a", "MORE", "v", "alice"))
        .unwrap_err();
    assert!(e.is_quota(), "{e}");
    let _ok = m
        .run_unproven(set_secret("a", "API_KEY", "again", "alice"))
        .unwrap();
    let _ok = m.run_unproven(withdraw_app("a", "more", "alice")).unwrap();
    for i in 1..MAX_SECRETS_PER_DEPLOYMENT {
        let _ok = m
            .run_unproven(clear_secret("a", &format!("FILLER_{i}"), "alice"))
            .unwrap();
    }

    let _ok = m
        .run_unproven(instance("i1", "bob", "b-1.0.0", Some("a")))
        .unwrap();
    let _ok = m
        .run_unproven(instance("i2", "bob", "b-1.0.0", None))
        .unwrap();
    let of = |m: &SystemMachine, app: &str| {
        m.state
            .deployment_secrets_of(app, "p")
            .and_then(|h| h.get("API_KEY"))
            .map(|s| s.value.0.clone())
    };
    assert_eq!(of(&m, "i1").as_deref(), Some("again"));
    assert_eq!(of(&m, "i2"), None, "created from no deployment");

    // Clear: only a manager, only what it holds.
    assert!(m.run_unproven(clear_secret("a", "API_KEY", "bob")).is_err());
    assert_eq!(
        m.run_unproven(clear_secret("a", "OTHER", "alice"))
            .unwrap_err(),
        NO_DEPLOYMENT_SECRET
    );
    let _ok = m
        .run_unproven(clear_secret("a", "API_KEY", "alice"))
        .unwrap();
    assert_eq!(of(&m, "i1"), None);
    assert!(!m.state.deployment_secrets.contains_key("a"));

    // An offer moved to a release that does not declare it: the value stays,
    // unused by such an instance (`component::run_secrets`), until cleared.
    let _ok = m
        .run_unproven(set_secret("a", "API_KEY", "kept", "alice"))
        .unwrap();
    let _ok = m
        .run_unproven(offer_app("a", "notes", "2.0.0", "alice"))
        .unwrap();
    assert!(!m
        .state
        .deployment_declares(&m.state.deployments["a"], "API_KEY"));
    assert_eq!(of(&m, "i1").as_deref(), Some("kept"));

    // Deleting the deployment drops its values.
    let _gone = m.state.apps.remove("i1");
    let _ok = m.run_unproven(withdraw_app("a", "notes", "alice")).unwrap();
    let _ok = m.run_unproven(delete_deployment("a", "alice")).unwrap();
    assert!(!m.state.deployment_secrets.contains_key("a"));
    assert!(m.state.deployment_secrets.contains_key("o"));
}

/// Setting or clearing a deployment's secret carries a manager's passkey
/// consent over exactly that change, once; the consent names the value's
/// fingerprint, worked out where it applies, so no node swaps the value.
#[test]
fn deployment_secrets_carry_their_managers_consent() {
    let (mut m, mut alice, mut bob) = two_accounts();
    let _ok = m.run_unproven(install_declaring("alice", "1.0.0")).unwrap();
    let _ok = m
        .run_unproven(create_deployment(
            "d",
            super::Deployer::Account("alice".into()),
            "alice",
        ))
        .unwrap();
    let _ok = m
        .run_unproven(offer_app("d", "notes", "1.0.0", "alice"))
        .unwrap();
    let set =
        |actor: &str, value: &str, consent: serde_json::Value| SystemCmd::SetDeploymentSecret {
            deployment: "d".into(),
            name: "API_KEY".into(),
            value: crate::component::SecretValue(value.into()),
            actor: actor.into(),
            at: 20,
            consent,
            used: false,
        };
    let fp = |v: &str| crate::component::SecretValue(v.into()).fingerprint();
    let args = json!({ "name": "API_KEY", "fingerprint": fp("real") });
    assert!(m
        .run(set("alice", "real", serde_json::Value::Null))
        .is_err());
    let c = consent(&mut bob, "deployment:d", "__set_secret", &args);
    assert!(m.run(set("alice", "real", c)).is_err(), "bob's passkey");
    let c = consent(&mut bob, "deployment:d", "__set_secret", &args);
    assert!(m.run(set("bob", "real", c)).is_err(), "bob manages nothing");
    let c = consent(&mut alice, "deployment:other", "__set_secret", &args);
    assert!(
        m.run(set("alice", "real", c)).is_err(),
        "another deployment"
    );
    let c = consent(&mut alice, "deployment:d", "__clear_secret", &args);
    assert!(m.run(set("alice", "real", c)).is_err(), "a clear");
    let c = consent(&mut alice, "deployment:d", "__set_secret", &args);
    assert!(
        m.run(set("alice", "swapped", c.clone())).is_err(),
        "another value under the consent"
    );
    assert!(m.state.deployment_secrets.is_empty());
    let _ok = m.run(set("alice", "real", c.clone())).unwrap();
    assert!(m.run(set("alice", "real", c)).is_err(), "used once");

    let clear = |consent: serde_json::Value| SystemCmd::ClearDeploymentSecret {
        deployment: "d".into(),
        name: "API_KEY".into(),
        actor: "alice".into(),
        consent,
    };
    let c = consent(
        &mut alice,
        "deployment:d",
        "__clear_secret",
        &json!({ "name": "OTHER" }),
    );
    assert!(m.run(clear(c)).is_err(), "another name");
    let c = consent(
        &mut alice,
        "deployment:d",
        "__clear_secret",
        &json!({ "name": "API_KEY" }),
    );
    let _ok = m.run(clear(c)).unwrap();
    assert!(m.state.deployment_secrets.is_empty());
}

/// A deployment's secret value is in no `Debug` print of the state or of the
/// command, and in no answer `apply` gives.
#[test]
fn a_deployment_secret_value_never_prints() {
    const VALUE: &str = "never-printed-value";
    let mut m = deploying(super::Deployments::Open);
    let _ok = m.run_unproven(install_declaring("alice", "1.0.0")).unwrap();
    let _ok = m
        .run_unproven(create_deployment(
            "a",
            super::Deployer::Account("alice".into()),
            "alice",
        ))
        .unwrap();
    let _ok = m
        .run_unproven(offer_app("a", "notes", "1.0.0", "alice"))
        .unwrap();
    let cmd = set_secret("a", "API_KEY", VALUE, "alice");
    assert!(!format!("{cmd:?}").contains(VALUE));
    let out = m.run_unproven(cmd).unwrap();
    assert!(!out.to_string().contains(VALUE), "{out}");
    let refused = m
        .run_unproven(set_secret("a", "OTHER", VALUE, "alice"))
        .unwrap_err();
    assert!(!refused.message().contains(VALUE));
    assert!(!format!("{:?}", m.state).contains(VALUE));
    assert!(!format!("{:?}", m.state.deployment_secrets).contains(VALUE));
    // The snapshot does carry it: the system group's state is sealed at rest.
    assert!(serde_json::to_string(&m.state).unwrap().contains(VALUE));
}

/// A system state written before deployment secrets (log entries and
/// snapshots are JSON) reads with none, and writes back as it was.
#[test]
fn state_from_before_deployment_secrets_reads_with_none_and_writes_as_before() {
    let mut m = deploying(super::Deployments::Open);
    let _ok = m.run_unproven(install_declaring("alice", "1.0.0")).unwrap();
    let _ok = m
        .run_unproven(create_deployment(
            "a",
            super::Deployer::Account("alice".into()),
            "alice",
        ))
        .unwrap();
    let before = serde_json::to_value(&m.state).unwrap();
    assert!(before.get("deploymentSecrets").is_none(), "{before}");
    let restored: SystemState = serde_json::from_value(before.clone()).unwrap();
    assert!(restored.deployment_secrets.is_empty());
    assert_eq!(serde_json::to_value(&restored).unwrap(), before);
    // A deletion logged before them applies as it did.
    let mut m = SystemMachine {
        state: restored,
        ..SystemMachine::default()
    };
    let _ok = m.run_unproven(delete_deployment("a", "alice")).unwrap();
    assert!(m.state.deployments.is_empty());
}

/// `package@version` installed by alice, declaring `secrets` (each bound to an
/// `x-<name>` header of `api.example.com`) and referencing `uses` (the manifest's JSON for them).
fn install_release(package: &str, version: &str, secrets: &[&str], uses: Value) -> SystemCmd {
    let secrets: serde_json::Map<String, Value> = secrets
        .iter()
        .map(|n| {
            (
                (*n).to_owned(),
                json!({ "host": "api.example.com", "paths": ["/"], "header": format!("x-{}", n.to_ascii_lowercase()) }),
            )
        })
        .collect();
    let uses = if uses.is_null() { json!({}) } else { uses };
    SystemCmd::InstallPackage {
        package: package.into(),
        signer_id: "did:key:z".into(),
        release: serde_json::from_value(json!({
            "version": version, "mpk": format!("{package}-{version}"), "mpkSize": 1,
            "source": "upload", "services": { "": format!("b-{package}-{version}") },
            "installedBy": "alice", "installedAt": 5,
            "outbound": ["https://api.example.com/"], "secrets": secrets, "uses": uses,
        }))
        .unwrap(),
    }
}

/// A deployment offering `notes`, whose release references (`uses`) the package `pi`, holds
/// a value of a secret a release of `pi` in the reference's range declares, by an entry that
/// says it was judged so (`used`); not of a name nothing it reaches declares, nor of one a
/// release outside the range declares. The app it creates, carrying the deployment, reads the
/// value for a name its own release declares, and `notes` itself does not.
#[test]
fn a_deployment_holds_secrets_the_releases_its_offers_use_declare() {
    use std::collections::BTreeMap;

    use super::{Deployer, Deployments, UNDECLARED_SECRET};
    let mut m = deploying(Deployments::Open);
    let _ok = m
        .run_unproven(install_release(
            "pi",
            "1.0.0",
            &["NEAR_AI_API_KEY"],
            Value::Null,
        ))
        .unwrap();
    let _ok = m
        .run_unproven(install_release("pi", "2.0.0", &["V2_KEY"], Value::Null))
        .unwrap();
    let _ok = m
        .run_unproven(install_release(
            "notes",
            "1.0.0",
            &[],
            json!({ "inference": { "package": "pi", "range": "^1", "version": "1.0.0" } }),
        ))
        .unwrap();
    let _ok = m
        .run_unproven(create_deployment(
            "a",
            Deployer::Account("alice".into()),
            "alice",
        ))
        .unwrap();
    let _ok = m
        .run_unproven(SystemCmd::OfferApp {
            deployment: "a".into(),
            app: "notes".into(),
            package: "notes".into(),
            version: "1.0.0".into(),
            actor: "alice".into(),
            at: 10,
            consent: Value::Null,
        })
        .unwrap();
    let d = m.state.deployments["a"].clone();
    assert!(!m.state.deployment_declares(&d, "NEAR_AI_API_KEY"));
    assert!(m.state.deployment_uses_declare(&d, "NEAR_AI_API_KEY"));

    // An entry judged by the offered releases alone (every one from before) refuses it.
    assert_eq!(
        m.run_unproven(set_secret("a", "NEAR_AI_API_KEY", "k", "alice"))
            .unwrap_err(),
        UNDECLARED_SECRET
    );
    // Nothing it reaches declares OTHER; pi 2.0.0 is outside `^1`.
    for name in ["OTHER", "V2_KEY"] {
        assert_eq!(
            m.run_unproven(set_used_secret("a", name, "k", "alice"))
                .unwrap_err(),
            UNDECLARED_SECRET,
            "{name}"
        );
    }
    // Only a manager.
    assert!(m
        .run_unproven(set_used_secret("a", "NEAR_AI_API_KEY", "k", "bob"))
        .is_err());
    assert!(m.state.deployment_secrets.is_empty());
    let out = m
        .run_unproven(set_used_secret("a", "NEAR_AI_API_KEY", "devs-key", "alice"))
        .unwrap();
    assert!(!out.to_string().contains("devs-key"), "{out}");

    // A release installed later inside the range is reached too.
    let _ok = m
        .run_unproven(install_release("pi", "1.1.0", &["NEW_KEY"], Value::Null))
        .unwrap();
    let _ok = m
        .run_unproven(set_used_secret("a", "NEW_KEY", "new", "alice"))
        .unwrap();
    let declarers = m.state.deployment_declarers(&m.state.deployments["a"]);
    assert_eq!(
        declarers["NEAR_AI_API_KEY"].declared_by,
        [("pi".to_owned(), "1.0.0".to_owned())].into()
    );
    assert_eq!(
        declarers["NEAR_AI_API_KEY"].apps,
        ["notes".to_owned()].into()
    );
    assert!(!declarers.contains_key("V2_KEY"));

    // The app `notes` creates, carrying the deployment, gets the value for a name its own
    // release declares; `notes`, declaring none, gets nothing of it.
    for (id, bundle) in [("member", "b-pi-1.0.0"), ("notes-1", "b-notes-1.0.0")] {
        let _prev = m.state.apps.insert(
            id.into(),
            serde_json::from_value(json!({
                "appId": id, "name": id, "group": 2, "bundle": bundle, "owner": "alice",
                "org": p("alice"), "replicas": [1], "createdAt": 10, "deployment": "a",
            }))
            .unwrap(),
        );
    }
    let release = |package: &str| m.state.packages[package].versions["1.0.0"].secrets.clone();
    let filled = |app: &str, package: &str| {
        crate::component::run_secrets(
            &BTreeMap::new(),
            m.state.deployment_secrets_of(app, package),
            &release(package),
        )
        .0
        .into_iter()
        .map(|(name, value)| (name, value.as_str().to_owned()))
        .collect::<BTreeMap<String, String>>()
    };
    assert_eq!(
        filled("member", "pi"),
        [("NEAR_AI_API_KEY".to_owned(), "devs-key".to_owned())].into()
    );
    assert!(filled("notes-1", "notes").is_empty());
}

/// A unit member a deployment's instance creates from a release of its offer's own signer
/// (`api::apps::unit_members`) reads the deployment's secret, though no one acting for the
/// deployment's owner may use that release.
#[test]
fn a_member_of_its_offers_signer_reads_the_deployments_secret() {
    use super::{cluster_version::Feature, Deployer, Deployments, Visibility};
    let mut m = deploying(Deployments::Open);
    m.state.cluster_version = Some(Feature::PrivateReleases.since().to_owned());
    let uses = json!({ "inference": { "package": "pi", "range": "*" } });
    for cmd in [
        install_release("pi", "1.0.0", &["PI_KEY"], Value::Null),
        install_release("notes", "1.0.0", &["PI_KEY"], uses),
        create_deployment("a", Deployer::Account("alice".into()), "alice"),
        SystemCmd::OfferApp {
            deployment: "a".into(),
            app: "notes".into(),
            package: "notes".into(),
            version: "1.0.0".into(),
            actor: "alice".into(),
            at: 10,
            consent: Value::Null,
        },
        set_secret("a", "PI_KEY", "devs-key", "alice"),
    ] {
        let _ok = m.run_unproven(cmd).unwrap();
    }
    // pi becomes private to an organization alice is not in; its signer is notes'.
    if let Some(v) = m
        .state
        .packages
        .get_mut("pi")
        .and_then(|p| p.versions.get_mut("1.0.0"))
    {
        v.visibility = Some(Visibility::Private);
        v.orgs = std::collections::BTreeSet::from(["elsewhere".to_owned()]);
    }
    let _prev = m.state.apps.insert(
        "member".into(),
        serde_json::from_value(json!({
            "appId": "member", "name": "member", "group": 2, "bundle": "b-pi-1.0.0",
            "owner": "bob", "org": p("bob"), "replicas": [1], "createdAt": 10,
            "deployment": "a", "release": { "package": "pi", "version": "1.0.0" },
        }))
        .unwrap(),
    );
    assert!(m.state.deployment_secrets_of("member", "pi").is_some());
}

/// A deployment holds no secret a used release declares that no one acting for its owner
/// may use, from the logged version that brings private releases; below it, as it applied.
#[test]
fn a_used_release_its_owner_may_not_use_declares_no_deployment_secret() {
    use super::{cluster_version::Feature, Deployer, Deployments, Visibility, UNDECLARED_SECRET};
    let mut m = deploying(Deployments::Open);
    let _ok = m
        .run_unproven(install_release("pi", "1.0.0", &["PI_KEY"], Value::Null))
        .unwrap();
    let uses = json!({ "inference": { "package": "pi", "range": "*" } });
    let _ok = m
        .run_unproven(install_release("notes", "1.0.0", &[], uses))
        .unwrap();
    let _ok = m
        .run_unproven(create_deployment(
            "a",
            Deployer::Account("alice".into()),
            "alice",
        ))
        .unwrap();
    let _ok = m
        .run_unproven(SystemCmd::OfferApp {
            deployment: "a".into(),
            app: "notes".into(),
            package: "notes".into(),
            version: "1.0.0".into(),
            actor: "alice".into(),
            at: 10,
            consent: Value::Null,
        })
        .unwrap();
    // pi, another publisher's, becomes private to an organization alice is not in.
    if let Some(pi) = m.state.packages.get_mut("pi") {
        pi.signer_id = "did:key:other".into();
        if let Some(v) = pi.versions.get_mut("1.0.0") {
            v.visibility = Some(Visibility::Private);
            v.orgs = std::collections::BTreeSet::from(["elsewhere".to_owned()]);
        }
    }
    let mut old = SystemMachine {
        state: m.state.clone(),
        ..Default::default()
    };
    assert!(old
        .run_unproven(set_used_secret("a", "PI_KEY", "k", "alice"))
        .is_ok());
    m.state.cluster_version = Some(Feature::PrivateReleases.since().to_owned());
    assert_eq!(
        m.run_unproven(set_used_secret("a", "PI_KEY", "k", "alice"))
            .unwrap_err(),
        UNDECLARED_SECRET
    );
    assert!(m
        .state
        .deployment_declarers(&m.state.deployments["a"])
        .is_empty());
}

/// A release recorded before its `uses` were kept reads with none and writes as it did; an
/// install of the same `.mpk` again fills them in, and nothing else of the record changes.
/// An entry from before `used` reads as `false` and writes as it was.
#[test]
fn releases_and_entries_from_before_used_secrets_replay_unchanged() {
    use super::Deployments;
    let mut m = deploying(Deployments::Open);
    let _ok = m
        .run_unproven(install_release("notes", "1.0.0", &[], Value::Null))
        .unwrap();
    let before = serde_json::to_value(&m.state).unwrap();
    let release = &before["packages"]["notes"]["versions"]["1.0.0"];
    assert!(release.get("uses").is_none(), "{release}");
    let restored: SystemState = serde_json::from_value(before.clone()).unwrap();
    assert!(restored.packages["notes"].versions["1.0.0"].uses.is_empty());
    assert_eq!(serde_json::to_value(&restored).unwrap(), before);

    let uses = json!({ "inference": { "package": "pi", "range": "^1" } });
    let _ok = m
        .run_unproven(install_release("notes", "1.0.0", &[], uses.clone()))
        .unwrap();
    let mut after = serde_json::to_value(&m.state).unwrap();
    assert_eq!(
        after["packages"]["notes"]["versions"]["1.0.0"]["uses"],
        uses
    );
    let _gone = after["packages"]["notes"]["versions"]["1.0.0"]
        .as_object_mut()
        .unwrap()
        .remove("uses");
    assert_eq!(after, before, "only the uses were filled in");

    let entry = json!({ "op": "setDeploymentSecret", "deployment": "d", "name": "API_KEY",
        "value": "v", "actor": "a", "at": 1 });
    let cmd: SystemCmd = serde_json::from_value(entry.clone()).unwrap();
    assert!(matches!(
        cmd,
        SystemCmd::SetDeploymentSecret { used: false, .. }
    ));
    let written = serde_json::to_value(&cmd).unwrap();
    assert!(written.get("used").is_none(), "{written}");
    let mut entry = entry;
    entry["consent"] = Value::Null;
    assert_eq!(written, entry);
}

/// A release's instance mode (`PackageVersion::instance_org`, the manifest's `instanceOrg`)
/// is kept on its record, and left out of a record without it, which reads and writes as it
/// did; installing the same release again with it fills it in and changes nothing else.
/// The entry that carries it is `Feature::OrdinaryOrgs`.
#[test]
fn a_releases_instance_mode_is_kept_and_filled_in_on_a_record_from_before() {
    use super::cluster_version::Feature;
    use super::Deployments;
    let mut m = deploying(Deployments::Open);
    let plain = install_release("notes", "1.0.0", &[], Value::Null);
    assert!(plain.features().is_empty());
    let _ok = m.run_unproven(plain).unwrap();
    let before = serde_json::to_value(&m.state).unwrap();
    let release = &before["packages"]["notes"]["versions"]["1.0.0"];
    assert!(release.get("instanceOrg").is_none(), "{release}");
    let restored: SystemState = serde_json::from_value(before.clone()).unwrap();
    assert_eq!(
        restored.packages["notes"].versions["1.0.0"].instance_org,
        None
    );
    assert_eq!(serde_json::to_value(&restored).unwrap(), before);

    let mut again = install_release("notes", "1.0.0", &[], Value::Null);
    if let SystemCmd::InstallPackage { release, .. } = &mut again {
        release.instance_org = Some(calimero_bundle::InstanceOrg::Standalone);
    }
    assert_eq!(again.features(), [Feature::OrdinaryOrgs]);
    let _ok = m.run_unproven(again).unwrap();
    let mut after = serde_json::to_value(&m.state).unwrap();
    assert_eq!(
        after["packages"]["notes"]["versions"]["1.0.0"]["instanceOrg"],
        "standalone"
    );
    let _gone = after["packages"]["notes"]["versions"]["1.0.0"]
        .as_object_mut()
        .unwrap()
        .remove("instanceOrg");
    assert_eq!(after, before, "only the mode was filled in");
}

/// An app of `org` (or of alice's personal organization, `p-alice`) in a
/// machine whose file quota is `quota`.
fn machine_with_apps(quota: u64, apps: &[(&str, Option<&str>)]) -> SystemMachine {
    let mut m = SystemMachine::default();
    m.state.quotas.max_file_bytes_per_org = quota;
    for (id, org) in apps {
        let org = &Some(org.unwrap_or("p-alice"));
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
                    "orgId": org, "name": org, "members": { "alice": "owner" }, "teams": {},
                    "personal": *org == "p-alice",
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
    assert!(refused.is_quota(), "{refused}");
    assert_eq!(m.state.file_bytes("org"), 100, "a refusal changes nothing");

    // Another payer has its own quota.
    let _ok = m.run_unproven(charge("c", 100)).unwrap();
    assert_eq!(m.state.file_bytes("p-alice"), 100);
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
            members: None,
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
    // To bob's personal organization, which pays its files now.
    assert_eq!(m.state.file_bytes("p-bob"), 70);
    assert_eq!(m.state.file_bytes("p-alice"), 0);
    let _ok = m
        .run_unproven(SystemCmd::DeleteApp {
            app_id: "a".into(),
            actor: "alice".into(),
            owner_in_org: true,
            owner_in_team: true,
            members: None,
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
    bobs.org = "p-bob".into();
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
    assert!(refused.is_quota(), "{refused}");
    assert_eq!(m.state.apps["a"].owner, "alice");
    let _ok = m.run_unproven(charge("b", -50)).unwrap();
    let _ok = m.run_unproven(transfer).unwrap();
    assert_eq!(m.state.file_bytes("p-bob"), 80);
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
            .message()
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
            .message()
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
            personal: false,
            app_team: None,
            capabilities: None,
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
            .message()
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
    .is_quota());
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
        .message()
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
        .is_quota());
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
    let _ = m.state.apps.get_mut("app").map(|a| a.org = "org".into());
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
        Governance, Rejection, RejectionKind, SignUp, SignUpInvite, SignUpRedemption, SystemCmd,
        SystemMachine, SystemState, OPERATOR, SIGN_UP_PREFIX,
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
                clamp_grants: false,
            }),
            refresh,
            consent,
            personal_org: None,
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

    /// `cmd`, a sign-up, as the API logs it once every node reads the flag.
    fn clamped(mut cmd: SystemCmd) -> SystemCmd {
        if let SystemCmd::CreateAccount {
            invite: Some(r), ..
        } = &mut cmd
        {
            r.clamp_grants = true;
        }
        cmd
    }

    /// Signing up with an organization invite that gives every app added
    /// later grants it only while the invite's creator is still an admin,
    /// as redeeming it does; an entry without the flag grants as minted.
    #[test]
    fn a_sign_up_with_an_all_apps_invite_is_clamped_like_a_redeem() {
        let granted = |clamp: bool, creator_role: &str, id: &str| {
            let mut m = machine(SignUp::Open);
            org_invite(&mut m, "inv_x");
            let o = m.state.orgs.get_mut("org").unwrap();
            let _prev = o.members.insert(
                "alice".into(),
                serde_json::from_value(json!(creator_role)).unwrap(),
            );
            let invite = m
                .state
                .invites
                .get_mut(&crate::proof::invite_key("inv_x"))
                .unwrap();
            invite.all_apps = true;
            invite.created_by = "alice".into();
            let cmd = signing_up(id, Some("inv_x"), 10);
            let cmd = if clamp { clamped(cmd) } else { cmd };
            let _ok = m.run(cmd).unwrap();
            assert!(m.state.orgs["org"].members.contains_key(id), "joined");
            m.state.orgs["org"].all_apps.contains(id)
        };
        assert!(granted(true, "owner", "c"), "the creator still owns it");
        assert!(!granted(true, "member", "c"), "the creator was demoted");
        assert!(
            granted(false, "member", "c"),
            "as an earlier release logged it"
        );

        let plain = signing_up("c", Some("inv_x"), 10);
        assert!(plain.features().is_empty());
        assert_eq!(
            clamped(plain).features(),
            [super::super::cluster_version::Feature::InviteCapabilities]
        );
    }

    fn refused(r: Result<serde_json::Value, Rejection>) -> Rejection {
        let e = r.unwrap_err();
        assert_eq!(e.kind(), RejectionKind::SignUpRefused, "{e}");
        assert!(e.message().starts_with(SIGN_UP_PREFIX), "{e}");
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
        assert!(e.message().contains("invite only"), "{e}");
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
                clamp_grants: false,
            }),
            refresh,
            consent,
            personal_org: None,
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
        assert!(e.message().contains("pre-authorized"), "{e}");
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
        assert!(e.message().contains("owner"), "{e}");
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

fn refused(m: &mut SystemMachine, cmd: SystemCmd) -> super::Rejection {
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
    org_app.org = "org".into();
    let _prev = m.state.apps.insert("oa".into(), org_app);
    // Alice's app made without an organization is in her personal one.
    assert_eq!(
        m.state.backup_holder(&m.state.apps["app"]),
        BackupHolder::Org(p("alice"))
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
        "owner": "alice", "org": p("alice"), "replicas": [1], "createdAt": 0
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
    org_app.org = "org".into();
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

/// A consent-page `createOrg` asked from an app's frontend: the organization
/// and that package's app team in one entry, open (standalone), the creator
/// its lead and its members able to make channels below it; the team's id
/// comes from the org's, so every replica makes the same one.
#[test]
fn an_organization_made_from_an_app_starts_with_its_app_team() {
    use crate::types::{caps, OrgMode};

    let package = |m: &mut SystemMachine| {
        let _prev = m.state.packages.insert(
            "com.chat".into(),
            serde_json::from_value(json!({
                "name": "com.chat", "signerId": "did:key:z", "owner": "alice",
                "versions": { "1.0.0": {
                    "version": "1.0.0", "mpk": "m", "mpkSize": 1, "source": "upload",
                    "services": { "": "b" }, "installedBy": "alice", "installedAt": 0,
                    "metadata": { "name": "Chat" }
                } }
            }))
            .unwrap(),
        );
    };
    let create = |id: &str, app_team: Option<&str>| SystemCmd::CreateOrg {
        org_id: id.into(),
        name: "Acme".into(),
        actor: "alice".into(),
        release: None,
        recovery_key: None,
        at: 0,
        name_fits: true,
        personal: false,
        app_team: app_team.map(str::to_owned),
        capabilities: None,
    };
    let (mut m, _alice, _bob, _carol) = proven_cluster();
    let (mut other, _a, _b, _c) = proven_cluster();
    package(&mut m);
    package(&mut other);

    let made = m.run_unproven(create("acme", Some("com.chat"))).unwrap();
    let again = other
        .run_unproven(create("acme", Some("com.chat")))
        .unwrap();
    assert_eq!(made, again, "every replica makes the same team");
    let team_id = made["teamId"].as_str().unwrap().to_owned();
    assert_eq!(made["orgId"], json!("acme"));
    let o = &m.state.orgs["acme"];
    assert_eq!(o.mode(), OrgMode::Standalone);
    let t = o.app_team("com.chat").expect("the app team");
    assert_eq!(t.team_id, team_id);
    assert_eq!(t.name, "Chat");
    assert_eq!(t.join, TeamJoin::Everyone);
    assert_eq!(t.members.get("alice"), Some(&true), "the creator leads it");
    assert_eq!(
        t.default_capabilities,
        Some(caps::INVITE_MEMBERS | caps::CREATE_CHILD_TEAM)
    );

    // An unknown package takes nothing: no organization either.
    assert!(m.run_unproven(create("ghost", Some("com.ghost"))).is_err());
    assert!(!m.state.orgs.contains_key("ghost"));

    // Without it the entry and the organization are as before.
    let plain = create("plain", None);
    let entry = serde_json::to_value(&plain).unwrap();
    assert!(entry.get("app_team").is_none(), "{entry}");
    assert_eq!(m.run_unproven(plain).unwrap(), json!({ "orgId": "plain" }));
    assert_eq!(m.state.orgs["plain"].mode(), OrgMode::Legacy);
}

/// A `CreateOrg` naming `capabilities` sets the organization's member default
/// in the same entry (with or without an app team), so every replica applies
/// it alike; without it the entry, the default and an earlier release's log
/// entry are as before.
#[test]
fn an_organization_created_with_capabilities_sets_its_member_default() {
    use crate::types::caps;

    let wanted = caps::CREATE_APP | caps::INVITE_MEMBERS | caps::JOIN_OPEN_TEAMS;
    let create = |id: &str, capabilities: Option<u32>| SystemCmd::CreateOrg {
        org_id: id.into(),
        name: "Acme".into(),
        actor: "alice".into(),
        release: None,
        recovery_key: None,
        at: 0,
        name_fits: true,
        personal: false,
        app_team: None,
        capabilities,
    };
    let (mut m, _alice, _bob, _carol) = proven_cluster();
    let (mut other, _a, _b, _c) = proven_cluster();

    let _ok = m.run_unproven(create("acme", Some(wanted))).unwrap();
    let _ok = other.run_unproven(create("acme", Some(wanted))).unwrap();
    assert_eq!(m.state.orgs["acme"].default_capabilities, wanted);
    assert_eq!(
        serde_json::to_value(&m.state.orgs["acme"]).unwrap(),
        serde_json::to_value(&other.state.orgs["acme"]).unwrap(),
        "every replica applies it alike"
    );

    // Zero is a choice too: members hold nothing by default.
    let _ok = m.run_unproven(create("none", Some(0))).unwrap();
    assert_eq!(m.state.orgs["none"].default_capabilities, 0);

    // Absent: the default is what a new organization holds, and the entry
    // does not name the field.
    let plain = create("plain", None);
    let entry = serde_json::to_value(&plain).unwrap();
    assert!(entry.get("capabilities").is_none(), "{entry}");
    assert!(plain.features().is_empty());
    let _ok = m.run_unproven(plain).unwrap();
    assert_eq!(m.state.orgs["plain"].default_capabilities, caps::DEFAULT);

    // An entry an earlier release logged has no field and replays unchanged.
    let old: SystemCmd = serde_json::from_value(json!({
        "op": "createOrg", "org_id": "old", "name": "N", "actor": "alice", "release": null,
    }))
    .unwrap();
    assert!(matches!(
        old,
        SystemCmd::CreateOrg {
            capabilities: None,
            ..
        }
    ));
    let _ok = m.run_unproven(old).unwrap();
    assert_eq!(m.state.orgs["old"].default_capabilities, caps::DEFAULT);
}

/// `actor` mints an invite of `org` under `token` carrying `capabilities`,
/// with their passkey's consent over exactly those terms.
fn mint_with_capabilities(
    m: &mut SystemMachine,
    pk: &mut Soft,
    actor: &str,
    token: &str,
    capabilities: Option<u32>,
) -> Result<serde_json::Value, super::Rejection> {
    let key = crate::proof::invite_key(token);
    let mut invite: crate::types::Invite = serde_json::from_value(json!({
        "org": "org", "role": "member", "expiresAt": 100, "usesLeft": 2,
    }))
    .unwrap();
    invite.capabilities = capabilities;
    let c = consent(
        pk,
        "org:org",
        "__create_invite",
        &super::invite_args(&key, &invite),
    );
    m.run(SystemCmd::CreateInvite {
        invite_key: key,
        invite,
        actor: actor.into(),
        consent: c,
        teams_seen: true,
    })
}

/// An invite carrying capabilities gives a new member exactly those bits as
/// their own setting; one without leaves them on the organization's default,
/// and a member already in keeps what they have.
#[test]
fn an_invite_carrying_capabilities_gives_them_to_a_new_member() {
    use crate::types::caps;

    let granted = caps::CREATE_APP | caps::INVITE_MEMBERS | caps::MANAGE_METADATA;
    let (mut m, mut alice, _bob, _carol) = proven_cluster();
    let _ok = mint_with_capabilities(&mut m, &mut alice, "alice", "inv_c", Some(granted)).unwrap();
    let _ok = mint_with_capabilities(&mut m, &mut alice, "alice", "inv_p", None).unwrap();
    let key = crate::proof::invite_key("inv_c");
    assert_eq!(m.state.invites[&key].capabilities, Some(granted));

    // An existing member redeeming it keeps their own setting.
    let _same = m.run(redemption("inv_c", "bob", 10)).unwrap();
    assert!(!m.state.orgs["org"].capabilities.contains_key("bob"));

    let _ok = m.run(redemption("inv_c", "carol", 11)).unwrap();
    let o = &m.state.orgs["org"];
    assert_eq!(o.capabilities["carol"], granted);
    assert_eq!(o.org_caps("carol"), granted);

    // Without capabilities the member is on the default, as before.
    let _gone = m.state.orgs.get_mut("org").unwrap().members.remove("carol");
    let _gone = m
        .state
        .orgs
        .get_mut("org")
        .unwrap()
        .capabilities
        .remove("carol");
    let _ok = m.run(redemption("inv_p", "carol", 12)).unwrap();
    let o = &m.state.orgs["org"];
    assert!(!o.capabilities.contains_key("carol"));
    assert_eq!(o.org_caps("carol"), o.default_capabilities);
}

/// Below admin, an invite grants only a subset of what its creator holds, so
/// every node refuses an over-grant alike; an admin may grant any bits.
#[test]
fn an_invite_never_grants_capabilities_its_creator_lacks() {
    use crate::types::caps;

    let (mut m, _alice, mut bob, _carol) = proven_cluster();
    let o = m.state.orgs.get_mut("org").unwrap();
    let _prev = o
        .capabilities
        .insert("bob".into(), caps::INVITE_MEMBERS | caps::CREATE_APP);

    let over = caps::CREATE_APP | caps::MANAGE_MEMBERS;
    assert_eq!(
        mint_with_capabilities(&mut m, &mut bob, "bob", "inv_over", Some(over)).unwrap_err(),
        super::CANNOT_GRANT_UNHELD.to_owned()
    );
    assert!(!m
        .state
        .invites
        .contains_key(&crate::proof::invite_key("inv_over")));
    let _ok =
        mint_with_capabilities(&mut m, &mut bob, "bob", "inv_ok", Some(caps::CREATE_APP)).unwrap();
    let _ok = mint_with_capabilities(&mut m, &mut bob, "bob", "inv_none", Some(0)).unwrap();

    let _prev = m
        .state
        .orgs
        .get_mut("org")
        .unwrap()
        .members
        .insert("bob".into(), OrgRole::Admin);
    let _ok = mint_with_capabilities(&mut m, &mut bob, "bob", "inv_admin", Some(over)).unwrap();
}

/// An invite from before the field has none and writes without it; the
/// consent names it only when it is given.
#[test]
fn an_invite_without_capabilities_reads_writes_and_is_consented_as_before() {
    let old: crate::types::Invite = serde_json::from_value(json!({
        "org": "org", "role": "member", "expiresAt": 1, "usesLeft": 1,
    }))
    .unwrap();
    assert_eq!(old.capabilities, None);
    assert!(serde_json::to_value(&old)
        .unwrap()
        .get("capabilities")
        .is_none());
    assert!(super::invite_args("k", &old).get("capabilities").is_none());
    let mut with = old;
    with.capabilities = Some(3);
    assert_eq!(super::invite_args("k", &with)["capabilities"], json!(3));
    assert_eq!(
        serde_json::to_value(&with).unwrap()["capabilities"],
        json!(3)
    );
}

/// What a creator no longer holds is not handed out by their invite: the
/// bits are clamped at redeem to what the creator may grant now, and a
/// creator who left the organization grants no member-specific bits.
#[test]
fn an_invite_redeems_at_most_what_its_creator_may_grant_now() {
    use crate::types::caps;

    let wide = caps::CREATE_APP | caps::MANAGE_MEMBERS | caps::INVITE_MEMBERS;
    let minted = |token: &str| {
        let (mut m, _alice, mut bob, _carol) = proven_cluster();
        let o = m.state.orgs.get_mut("org").unwrap();
        let _prev = o.members.insert("bob".into(), OrgRole::Admin);
        let _ok = mint_with_capabilities(&mut m, &mut bob, "bob", token, Some(wide)).unwrap();
        m
    };

    // Demoted to a member holding one of the bits.
    let mut m = minted("inv_d");
    let o = m.state.orgs.get_mut("org").unwrap();
    let _prev = o.members.insert("bob".into(), OrgRole::Member);
    let _prev = o.capabilities.insert("bob".into(), caps::CREATE_APP);
    let _ok = m.run(redemption("inv_d", "carol", 10)).unwrap();
    assert_eq!(m.state.orgs["org"].capabilities["carol"], caps::CREATE_APP);

    // Stripped of every capability.
    let mut m = minted("inv_s");
    let o = m.state.orgs.get_mut("org").unwrap();
    let _prev = o.members.insert("bob".into(), OrgRole::Member);
    let _prev = o.capabilities.insert("bob".into(), 0);
    let _ok = m.run(redemption("inv_s", "carol", 10)).unwrap();
    assert_eq!(m.state.orgs["org"].capabilities["carol"], 0);

    // Still an admin: all of it.
    let mut m = minted("inv_a");
    let _ok = m.run(redemption("inv_a", "carol", 10)).unwrap();
    assert_eq!(m.state.orgs["org"].capabilities["carol"], wide);

    // Removed from the organization: the default applies.
    let mut m = minted("inv_r");
    let _gone = m.state.orgs.get_mut("org").unwrap().members.remove("bob");
    let _ok = m.run(redemption("inv_r", "carol", 10)).unwrap();
    assert!(!m.state.orgs["org"].capabilities.contains_key("carol"));
}

/// Bits no capability has are refused in the new fields, in the log too.
#[test]
fn capability_bits_outside_the_defined_set_are_refused() {
    use crate::types::caps;

    let unknown = caps::CREATE_APP | (1 << 20);
    let (mut m, mut alice, _bob, _carol) = proven_cluster();
    let create = SystemCmd::CreateOrg {
        org_id: "bad".into(),
        name: "Bad".into(),
        actor: "alice".into(),
        release: None,
        recovery_key: None,
        at: 0,
        name_fits: true,
        personal: false,
        app_team: None,
        capabilities: Some(unknown),
    };
    assert_eq!(
        m.run_unproven(create).unwrap_err(),
        super::UNKNOWN_CAPABILITIES
    );
    assert!(!m.state.orgs.contains_key("bad"));
    assert_eq!(
        mint_with_capabilities(&mut m, &mut alice, "alice", "inv_bad", Some(unknown)).unwrap_err(),
        super::UNKNOWN_CAPABILITIES
    );
    let _ok =
        mint_with_capabilities(&mut m, &mut alice, "alice", "inv_ok", Some(caps::DEFINED)).unwrap();
}

/// With an app team and capabilities in one entry both apply, and a refused
/// app team leaves no organization behind.
#[test]
fn an_organization_with_an_app_team_and_capabilities_is_all_or_nothing() {
    use crate::types::caps;

    let create = |id: &str, package: &str| SystemCmd::CreateOrg {
        org_id: id.into(),
        name: "Acme".into(),
        actor: "alice".into(),
        release: None,
        recovery_key: None,
        at: 0,
        name_fits: true,
        personal: false,
        app_team: Some(package.into()),
        capabilities: Some(caps::INVITE_MEMBERS),
    };
    let (mut m, _alice, _bob, _carol) = proven_cluster();
    let _prev = m.state.packages.insert(
        "com.chat".into(),
        serde_json::from_value(json!({
            "name": "com.chat", "signerId": "did:key:z", "owner": "alice",
            "versions": { "1.0.0": {
                "version": "1.0.0", "mpk": "m", "mpkSize": 1, "source": "upload",
                "services": { "": "b" }, "installedBy": "alice", "installedAt": 0,
                "metadata": { "name": "Chat" }
            } }
        }))
        .unwrap(),
    );
    let _ok = m.run_unproven(create("acme", "com.chat")).unwrap();
    let o = &m.state.orgs["acme"];
    assert_eq!(o.default_capabilities, caps::INVITE_MEMBERS);
    assert!(o.app_team("com.chat").is_some());

    assert!(m.run_unproven(create("ghost", "com.ghost")).is_err());
    assert!(!m.state.orgs.contains_key("ghost"));
}

/// A member already in the organization keeps their own capabilities when
/// they redeem an invite that carries others.
#[test]
fn an_existing_member_redeeming_a_capability_invite_keeps_their_own() {
    use crate::types::caps;

    let (mut m, mut alice, _bob, _carol) = proven_cluster();
    let _prev = m
        .state
        .orgs
        .get_mut("org")
        .unwrap()
        .capabilities
        .insert("bob".into(), caps::CREATE_TEAM);
    let _ok = mint_with_capabilities(&mut m, &mut alice, "alice", "inv_c", Some(caps::CREATE_APP))
        .unwrap();
    let _ok = m.run(redemption("inv_c", "bob", 10)).unwrap();
    assert_eq!(m.state.orgs["org"].capabilities["bob"], caps::CREATE_TEAM);
}

/// An invite's `all_apps` grant stands at redeem only while its creator is
/// still an owner or admin; an entry logged without the flag keeps granting
/// as minted, so every replica of an older log agrees.
#[test]
fn an_all_apps_invite_grants_only_while_its_creator_is_still_an_admin() {
    let minted = |token: &str| {
        let (mut m, _alice, _bob, _carol) = proven_cluster();
        let o = m.state.orgs.get_mut("org").unwrap();
        let _prev = o.members.insert("bob".into(), OrgRole::Admin);
        let invite: crate::types::Invite = serde_json::from_value(json!({
            "org": "org", "role": "member", "expiresAt": 100, "usesLeft": 1,
            "allApps": true, "createdBy": "bob", "capabilities": 3,
        }))
        .unwrap();
        let _prev = m
            .state
            .invites
            .insert(crate::proof::invite_key(token), invite);
        m
    };

    let mut m = minted("inv_a");
    let _ok = m.run(redemption("inv_a", "carol", 10)).unwrap();
    assert!(
        m.state.orgs["org"].all_apps.contains("carol"),
        "still admin"
    );

    let mut m = minted("inv_d");
    let _prev = m
        .state
        .orgs
        .get_mut("org")
        .unwrap()
        .members
        .insert("bob".into(), OrgRole::Member);
    let _ok = m.run(redemption("inv_d", "carol", 10)).unwrap();
    assert!(!m.state.orgs["org"].all_apps.contains("carol"), "demoted");
    // Both clamps at once: bob holds only the default, create-apps is gone.
    assert_eq!(
        m.state.orgs["org"].capabilities["carol"],
        3 & m.state.orgs["org"].default_capabilities
    );
    assert!(m.state.orgs["org"].members.contains_key("carol"), "joined");

    let mut m = minted("inv_r");
    let _gone = m.state.orgs.get_mut("org").unwrap().members.remove("bob");
    let _ok = m.run(redemption("inv_r", "carol", 10)).unwrap();
    assert!(!m.state.orgs["org"].all_apps.contains("carol"), "removed");
    assert!(!m.state.orgs["org"].capabilities.contains_key("carol"));

    // As an earlier release logged it: no flag, granted as minted.
    let mut m = minted("inv_o");
    let _gone = m.state.orgs.get_mut("org").unwrap().members.remove("bob");
    let old: SystemCmd = serde_json::from_value(json!({
        "op": "redeemInvite", "invite_key": crate::proof::invite_key("inv_o"),
        "account": "carol", "now": 10,
        "signature": crate::proof::sign_invite_redeem("inv_o", "carol", 10),
    }))
    .unwrap();
    assert!(old.features().is_empty());
    let _ok = m.run(old).unwrap();
    assert!(m.state.orgs["org"].all_apps.contains("carol"));
    assert_eq!(
        redemption("inv_o", "carol", 10).features(),
        [super::cluster_version::Feature::InviteCapabilities]
    );
}

/// A creator still in the organization with no capability row of their own
/// grants what the organization's default gives them.
#[test]
fn an_invite_is_clamped_to_the_default_when_its_creator_has_no_row() {
    use crate::types::caps;

    let (mut m, _alice, mut bob, _carol) = proven_cluster();
    let o = m.state.orgs.get_mut("org").unwrap();
    o.default_capabilities = caps::INVITE_MEMBERS | caps::CREATE_APP;
    assert!(!o.capabilities.contains_key("bob"));
    let _ok = mint_with_capabilities(
        &mut m,
        &mut bob,
        "bob",
        "inv_c",
        Some(caps::INVITE_MEMBERS | caps::CREATE_APP),
    )
    .unwrap();
    m.state.orgs.get_mut("org").unwrap().default_capabilities = caps::CREATE_APP;
    let _ok = m.run(redemption("inv_c", "carol", 10)).unwrap();
    assert_eq!(m.state.orgs["org"].capabilities["carol"], caps::CREATE_APP);
}

/// Once the logged cluster version reaches the release that clamps an
/// `all_apps` grant, every redemption is clamped whatever its entry's flag
/// says; before, an unflagged entry grants as minted. Read from state at
/// apply, so every replica agrees at each log position.
#[test]
fn the_all_apps_clamp_follows_the_logged_cluster_version() {
    let minted = |version: Option<&str>| {
        let (mut m, _alice, _bob, _carol) = proven_cluster();
        m.state.cluster_version = version.map(str::to_owned);
        let o = m.state.orgs.get_mut("org").unwrap();
        let _prev = o.members.insert("bob".into(), OrgRole::Member);
        let invite: crate::types::Invite = serde_json::from_value(json!({
            "org": "org", "role": "member", "expiresAt": 100, "usesLeft": 1,
            "allApps": true, "createdBy": "bob",
        }))
        .unwrap();
        let _prev = m
            .state
            .invites
            .insert(crate::proof::invite_key("inv_v"), invite);
        m
    };
    let unflagged = || -> SystemCmd {
        serde_json::from_value(json!({
            "op": "redeemInvite", "invite_key": crate::proof::invite_key("inv_v"),
            "account": "carol", "now": 10,
            "signature": crate::proof::sign_invite_redeem("inv_v", "carol", 10),
        }))
        .unwrap()
    };
    for (version, clamped) in [
        (None, false),
        (Some("1.0.0-rc.28"), false),
        (Some("1.0.0-rc.29"), true),
        (Some("1.0.0-rc.30"), true),
    ] {
        let mut m = minted(version);
        let _ok = m.run(unflagged()).unwrap();
        assert_eq!(
            m.state.orgs["org"].all_apps.contains("carol"),
            !clamped,
            "{version:?}"
        );
    }
}

fn delegate(source: &str, target: &str, methods: Option<&[&str]>, consent: Value) -> SystemCmd {
    SystemCmd::SetDelegation {
        account: "alice".into(),
        source: source.into(),
        target: target.into(),
        methods: methods.map(|m| m.iter().map(|x| (*x).to_owned()).collect()),
        consent,
    }
}

/// A person lets an app call another as them with their passkey over exactly the target and
/// methods; only apps they reach; a method list that is not one is refused; taking it back
/// needs no consent; deleting either app drops the grant.
#[test]
fn a_person_lets_an_app_act_as_them_with_their_passkey_and_takes_it_back() {
    let (mut m, mut alice, _bob) = moving();
    let app = |id: &str, owner: &str, access: Value| -> crate::types::AppRecord {
        serde_json::from_value(json!({
            "appId": id, "name": id, "group": 7, "bundle": "b1", "owner": owner,
            "org": "org", "grants": access, "replicas": [1, 2, 3], "createdAt": 0
        }))
        .unwrap()
    };
    let open = json!([{ "kind": "org" }]);
    let _prev = m
        .state
        .apps
        .insert("plugin".into(), app("plugin", "alice", open.clone()));
    let _prev = m.state.apps.insert("cal".into(), app("cal", "alice", open));
    let _prev = m.state.apps.insert(
        "closed".into(),
        app("closed", "bob", json!([{ "kind": "team", "team": "pt" }])),
    );
    let methods = ["get", "set"];
    let owned: Vec<String> = methods.iter().map(|x| (*x).to_owned()).collect();

    needs_consent(
        &mut m,
        &mut alice,
        ("plugin", "__delegate"),
        &super::delegation_args("cal", &owned),
        &super::delegation_args("cal", &["get".to_owned()]),
        |c| delegate("plugin", "cal", Some(&methods), c),
    );
    assert!(m.state.delegates("alice", "plugin", "cal", "set"));
    assert!(!m.state.delegates("alice", "plugin", "cal", "remove"));
    assert!(!m.state.delegates("bob", "plugin", "cal", "get"));

    // An app she does not reach, and a list that is not one of methods.
    let args = super::delegation_args("closed", &owned);
    let c = consent(&mut alice, "plugin", "__delegate", &args);
    assert!(m
        .run(delegate("plugin", "closed", Some(&methods), c))
        .is_err());
    let bad = ["__secret_set"];
    let args = super::delegation_args("cal", &["__secret_set".to_owned()]);
    let c = consent(&mut alice, "plugin", "__delegate", &args);
    assert!(m.run(delegate("plugin", "cal", Some(&bad), c)).is_err());

    // Taken back without a passkey.
    m.run(delegate("plugin", "cal", None, Value::Null)).unwrap();
    assert!(!m.state.delegates("alice", "plugin", "cal", "get"));
    assert!(m.state.delegations.is_empty());

    // Granted again, then the target goes: so does the grant.
    let c = consent(
        &mut alice,
        "plugin",
        "__delegate",
        &super::delegation_args("cal", &owned),
    );
    m.run(delegate("plugin", "cal", Some(&methods), c)).unwrap();
    m.state.remove_app("cal");
    assert!(m.state.delegations.is_empty());
}

fn link_app(name: &str, target: Option<&str>, consent: Value) -> SystemCmd {
    SystemCmd::LinkApp {
        app_id: "oa".into(),
        name: name.into(),
        target: target.map(Into::into),
        actor: "alice".into(),
        consent,
    }
}

/// An attach carries its manager's passkey consent over the name and the app, joins only
/// another app of the same organization, never replaces a unit member, and goes with the
/// attached app when that is deleted.
#[test]
fn an_app_is_attached_with_its_managers_consent_and_detached_with_it() {
    let (mut m, mut alice, _bob) = moving();
    let app = |id: &str, org: &str| -> crate::types::AppRecord {
        serde_json::from_value(json!({
            "appId": id, "name": id, "group": 6, "bundle": "b1", "owner": "alice",
            "org": org, "grants": [{ "kind": "org" }], "replicas": [1, 2, 3], "createdAt": 0
        }))
        .unwrap()
    };
    let _prev = m.state.apps.insert("ob".into(), app("ob", "org"));
    let _prev = m.state.apps.insert("nc".into(), app("nc", "new"));
    let _prev = m
        .state
        .apps
        .get_mut("oa")
        .unwrap()
        .links
        .insert("kv".into(), "ob".into());

    needs_consent(
        &mut m,
        &mut alice,
        ("oa", "__link"),
        &super::link_app_args("calendar", Some("ob")),
        &super::link_app_args("calendar", Some("nc")),
        |c| link_app("calendar", Some("ob"), c),
    );
    assert_eq!(m.state.apps["oa"].attached["calendar"], "ob");

    let args = super::link_app_args("calendar", Some("nc"));
    let c = consent(&mut alice, "oa", "__link", &args);
    assert!(
        m.run(link_app("calendar", Some("nc"), c)).is_err(),
        "another org"
    );
    let args = super::link_app_args("kv", Some("ob"));
    let c = consent(&mut alice, "oa", "__link", &args);
    assert!(
        m.run(link_app("kv", Some("ob"), c)).is_err(),
        "a unit member"
    );
    let args = super::link_app_args("Bad Name", Some("ob"));
    let c = consent(&mut alice, "oa", "__link", &args);
    assert!(
        m.run(link_app("Bad Name", Some("ob"), c)).is_err(),
        "a name"
    );

    let args = super::link_app_args("calendar", None);
    let c = consent(&mut alice, "oa", "__link", &args);
    m.run(link_app("calendar", None, c)).unwrap();
    assert!(m.state.apps["oa"].attached.is_empty());

    let args = super::link_app_args("calendar", Some("ob"));
    let c = consent(&mut alice, "oa", "__link", &args);
    m.run(link_app("calendar", Some("ob"), c)).unwrap();
    m.state.remove_app("ob");
    assert!(m.state.apps["oa"].attached.is_empty(), "gone with the app");
    assert_eq!(
        m.state.apps["oa"].links["kv"], "ob",
        "a member's link stays"
    );
}

/// Only a deployment's managers decide which release its instances run: an
/// `UpgradeApp` that says so (`in_deployment`), or any once the logged
/// cluster version brings the rule, moves an instance from a deployment only
/// to the release the deployment offers for its package; another package, a
/// raw bundle and a version not offered are refused `OUTSIDE_DEPLOYMENT`, and
/// once the managers offer a newer release the owner moves to it. An earlier
/// release's entry, without the flag below that version, replays as it
/// applied; a record it moved onto another package gets none of the
/// deployment's secrets ([`super::SystemState::deployment_secrets_of`]).
#[test]
fn an_instance_from_a_deployment_moves_only_to_what_it_offers() {
    use super::{cluster_version::Feature, Deployer, Deployments, OUTSIDE_DEPLOYMENT};
    let mut m = deploying(Deployments::Open);
    let _ok = m.run_unproven(install_declaring("alice", "1.0.0")).unwrap();
    let _ok = m.run_unproven(install_declaring("alice", "2.0.0")).unwrap();
    // Bob's own package, declaring the same secret, on WASM of his own.
    let _ok = m
        .run_unproven(SystemCmd::InstallPackage {
            package: "q".into(),
            signer_id: "did:key:bob".into(),
            release: serde_json::from_value(json!({
                "version": "1.0.0", "mpk": "q-1.0.0", "mpkSize": 1, "source": "upload",
                "services": { "": "b-q" }, "installedBy": "bob", "installedAt": 5,
                "outbound": ["https://attacker.example/"],
                "secrets": { "API_KEY": { "host": "attacker.example", "paths": ["/"], "header": "x-api-key" } },
            }))
            .unwrap(),
        })
        .unwrap();
    let _ok = m
        .run_unproven(create_deployment(
            "a",
            Deployer::Account("alice".into()),
            "alice",
        ))
        .unwrap();
    let _ok = m
        .run_unproven(offer_app("a", "notes", "1.0.0", "alice"))
        .unwrap();
    let _ok = m
        .run_unproven(set_secret("a", "API_KEY", "developers", "alice"))
        .unwrap();
    let d = m.state.deployments["a"].clone();
    assert!(m.state.deployment_covers(&d, "p"));
    assert!(!m.state.deployment_covers(&d, "q"));
    for id in ["i1", "i2", "i3"] {
        let _ok = m
            .run_unproven(instance(id, "bob", "b-1.0.0", Some("a")))
            .unwrap();
        m.state.apps.get_mut(id).unwrap().release = Some(PackageRef {
            package: "p".into(),
            version: "1.0.0".into(),
            service: None,
            compute: false,
        });
    }
    let up = |app: &str, bundle: &str, release: Option<(&str, &str)>, flag: bool| {
        let mut cmd = upgrade_to(app, "bob", bundle, release);
        if let SystemCmd::UpgradeApp { in_deployment, .. } = &mut cmd {
            *in_deployment = flag;
        }
        cmd
    };
    assert_eq!(
        up("i1", "b-q", Some(("q", "1.0.0")), true).features(),
        vec![Feature::UpgradeInDeployment]
    );
    assert!(up("i1", "b-q", Some(("q", "1.0.0")), false)
        .features()
        .is_empty());

    // An earlier release's entry (no flag, no cluster version that brings the
    // rule) replays as it applied: i2 moves onto bob's package. Its record now
    // names code the deployment does not offer, and gets none of its values.
    let _ok = m
        .run_unproven(up("i2", "b-q", Some(("q", "1.0.0")), false))
        .unwrap();
    assert_eq!(m.state.apps["i2"].release.as_ref().unwrap().package, "q");
    assert!(m.state.deployment_secrets_of("i2", "q").is_none());
    assert!(m.state.deployment_secrets_of("i1", "p").is_some());
    assert!(m.state.deployment_secrets_of("i1", "q").is_none());

    // Flagged: another package, a raw bundle, a version not offered.
    for (bundle, release) in [
        ("b-q", Some(("q", "1.0.0"))),
        ("b-2.0.0", None),
        ("b-2.0.0", Some(("p", "2.0.0"))),
    ] {
        assert_eq!(
            m.run_unproven(up("i1", bundle, release, true)).unwrap_err(),
            OUTSIDE_DEPLOYMENT,
            "{bundle}"
        );
    }
    // Once the cluster version brings the rule, an entry without the flag too.
    m.state.cluster_version = Some(Feature::UpgradeInDeployment.since().to_owned());
    assert_eq!(
        m.run_unproven(up("i3", "b-q", Some(("q", "1.0.0")), false))
            .unwrap_err(),
        OUTSIDE_DEPLOYMENT
    );
    assert_eq!(m.state.apps["i3"].release.as_ref().unwrap().package, "p");

    // The managers offer 2.0.0: the owner accepts it.
    let _ok = m
        .run_unproven(offer_app("a", "notes", "2.0.0", "alice"))
        .unwrap();
    let _ok = m
        .run_unproven(up("i1", "b-2.0.0", Some(("p", "2.0.0")), true))
        .unwrap();
    assert_eq!(
        m.state.apps["i1"].release.as_ref().unwrap().version,
        "2.0.0"
    );
    assert!(m.state.deployment_secrets_of("i1", "p").is_some());

    // A unit's member carries the deployment but runs a package it does not
    // offer: it moves within its own package (with its unit), never onto
    // another, and never as a standalone instance would.
    let q = |version: &str| PackageRef {
        package: "q".into(),
        version: version.into(),
        service: None,
        compute: false,
    };
    let p2 = PackageRef {
        package: "p".into(),
        version: "2.0.0".into(),
        service: None,
        compute: false,
    };
    let mut member = m.state.apps["i1"].clone();
    member.app_id = "m1".into();
    member.release = Some(q("1.0.0"));
    let _prev = m.state.apps.insert("m1".into(), member.clone());
    assert_eq!(
        m.state.within_deployment(&member, Some(&q("1.0.0")), None),
        Err(OUTSIDE_DEPLOYMENT),
        "not yet a member: the deployment offers no release of q"
    );
    let _prev = m
        .state
        .apps
        .get_mut("i1")
        .unwrap()
        .links
        .insert("store".into(), "m1".into());
    // A unit's run to `p@version`, naming the member's target.
    let unit_run = |id: &str, version: &str| crate::types::UpgradeRun {
        run_id: id.into(),
        package: "p".into(),
        version: version.into(),
        team: None,
        cascade: false,
        by: "bob".into(),
        started_at: 9,
        apps: vec!["m1".into(), "i1".into()],
        done: Vec::new(),
        failed: std::collections::BTreeMap::new(),
        finished: false,
        targets: std::iter::once((
            "m1".to_owned(),
            crate::types::RunTarget {
                package: "q".into(),
                version: "1.0.0".into(),
            },
        ))
        .collect(),
    };
    let org = member.org.clone();
    let runs = &mut m.state.orgs.get_mut(&org).unwrap().upgrade_runs;
    let _prev = runs.insert("offered".into(), unit_run("offered", "2.0.0"));
    let _prev = runs.insert("unoffered".into(), unit_run("unoffered", "1.0.0"));
    // With its unit, to the release the deployment offers: it moves within q.
    assert_eq!(
        m.state
            .within_deployment(&member, Some(&q("1.0.0")), Some("offered")),
        Ok(())
    );
    // Alone, or in a run to a release the developer did not offer: never.
    for run in [None, Some("unoffered"), Some("no-such-run")] {
        assert_eq!(
            m.state.within_deployment(&member, Some(&q("1.0.0")), run),
            Err(OUTSIDE_DEPLOYMENT),
            "{run:?}"
        );
    }
    // Onto another package, or a raw bundle, even with its unit: never.
    assert_eq!(
        m.state
            .within_deployment(&member, Some(&p2), Some("offered")),
        Err(OUTSIDE_DEPLOYMENT)
    );
    assert_eq!(
        m.state.within_deployment(&member, None, Some("offered")),
        Err(OUTSIDE_DEPLOYMENT)
    );
}

/// A deployment's offer moves an instance only while it serves: once the release it names is
/// one nobody who acts for the deployment's owner may use, an upgrade to it is refused
/// `OUTSIDE_DEPLOYMENT` from the logged version that brings private releases; an entry
/// logged below it replays as it applied.
#[test]
fn an_offer_that_no_longer_serves_moves_no_instance_from_the_version_that_brings_the_rule() {
    use super::{cluster_version::Feature, Deployer, Deployments, OUTSIDE_DEPLOYMENT};
    let mut m = deploying(Deployments::Open);
    let _ok = m.run_unproven(install_declaring("alice", "1.0.0")).unwrap();
    let _ok = m.run_unproven(install_declaring("alice", "2.0.0")).unwrap();
    let _ok = m
        .run_unproven(create_deployment(
            "a",
            Deployer::Account("alice".into()),
            "alice",
        ))
        .unwrap();
    let _ok = m
        .run_unproven(offer_app("a", "notes", "1.0.0", "alice"))
        .unwrap();
    let _ok = m
        .run_unproven(instance("i1", "bob", "b-1.0.0", Some("a")))
        .unwrap();
    m.state.apps.get_mut("i1").unwrap().release = Some(PackageRef {
        package: "p".into(),
        version: "1.0.0".into(),
        service: None,
        compute: false,
    });
    let _ok = m
        .run_unproven(offer_app("a", "notes", "2.0.0", "alice"))
        .unwrap();
    // 2.0.0 becomes private to an organization alice is not in: her offer no longer serves.
    if let Some(v) = m
        .state
        .packages
        .get_mut("p")
        .and_then(|p| p.versions.get_mut("2.0.0"))
    {
        v.visibility = Some(super::Visibility::Private);
        v.orgs = std::collections::BTreeSet::from(["elsewhere".to_owned()]);
    }
    let mut up = upgrade_to("i1", "bob", "b-2.0.0", Some(("p", "2.0.0")));
    if let SystemCmd::UpgradeApp { in_deployment, .. } = &mut up {
        *in_deployment = true;
    }
    let mut old = SystemMachine {
        state: m.state.clone(),
        ..Default::default()
    };
    old.state.cluster_version = Some(Feature::UpgradeInDeployment.since().to_owned());
    assert!(
        old.run_unproven(up.clone()).is_ok(),
        "replayed as it applied"
    );
    m.state.cluster_version = Some(Feature::PrivateReleases.since().to_owned());
    assert_eq!(m.run_unproven(up).unwrap_err(), OUTSIDE_DEPLOYMENT);
    assert_eq!(
        m.state.apps["i1"].release.as_ref().unwrap().version,
        "1.0.0"
    );
}

/// C8's fixture: `publishing`'s customer `cust` (cora its owner, eve an admin, dan a
/// member) with `inst` on `p@1.0.0`; `p@1.0.1` ships the same WASM and destinations
/// (green), `p@1.1.0` adds a telemetry host (red).
fn classified() -> SystemMachine {
    let (_key, signer) = publisher_key(41);
    let mut m = publishing(&signer, "owner");
    let _ok = m.run_unproven(account("eve")).unwrap();
    if let Some(o) = m.state.orgs.get_mut("cust") {
        let _prev = o.members.insert("eve".into(), crate::types::OrgRole::Admin);
    }
    if let Some(p) = m.state.packages.get_mut("p") {
        for (version, mpk, bundle, outbound) in [
            ("1.0.0", "00", "b1", json!(["https://ai.example/"])),
            ("1.0.1", "02", "b1", json!(["https://ai.example/"])),
            (
                "1.1.0",
                "01",
                "b2",
                json!(["https://ai.example/", "https://telemetry.example/"]),
            ),
        ] {
            let _prev = p.versions.insert(
                version.into(),
                serde_json::from_value(json!({
                    "version": version, "mpk": mpk, "mpkSize": 1, "source": "upload",
                    "services": { "": bundle }, "installedBy": "pam", "installedAt": 0,
                    "outbound": outbound
                }))
                .unwrap(),
            );
        }
    }
    instance_of(
        &mut m,
        "inst",
        "b1",
        json!({ "package": "p", "version": "1.0.0" }),
    );
    m
}

/// `actor`'s upgrade of `inst` to `version` carrying `class`, at `at`.
fn classified_upgrade(
    actor: &str,
    version: &str,
    class: Option<crate::types::UpgradeClass>,
    at: u64,
) -> SystemCmd {
    let bundle = if version == "1.1.0" { "b2" } else { "b1" };
    let mut cmd = upgrade_to("inst", actor, bundle, Some(("p", version)));
    if let SystemCmd::UpgradeApp {
        class: c,
        owner_approves,
        at: when,
        ..
    } = &mut cmd
    {
        *c = class;
        *owner_approves = true;
        *when = at;
    }
    cmd
}

fn green() -> crate::types::UpgradeClass {
    crate::types::UpgradeClass {
        class: calimero_bundle::update::Class::Green,
        reasons: Vec::new(),
    }
}

fn red(destinations: &[&str]) -> crate::types::UpgradeClass {
    crate::types::UpgradeClass {
        class: calimero_bundle::update::Class::Red,
        reasons: vec![calimero_bundle::update::Reason::EgressAdded {
            destinations: destinations.iter().map(|d| (*d).to_owned()).collect(),
        }],
    }
}

/// An update's class is judged again where it applies: a move the log shows adds a
/// destination is never applied as green (nor as red leaving that destination out, nor
/// as a class the classifier would never answer), and the class applied is kept on the
/// app's last upgrade. An entry without a class (an earlier release's, a raw bundle's)
/// applies as it did.
#[test]
fn an_updates_class_is_judged_against_the_log_and_recorded() {
    use super::cluster_version::Feature;
    use super::UPGRADE_CLASS_MISMATCH;
    let mut m = classified();
    assert_eq!(
        classified_upgrade("cora", "1.1.0", Some(green()), 10).features(),
        [Feature::OwnerApprovedUpgrades, Feature::UpgradeClasses]
    );
    for wrong in [
        green(),
        red(&["https://other.example/"]),
        crate::types::UpgradeClass {
            class: calimero_bundle::update::Class::Red,
            reasons: Vec::new(),
        },
    ] {
        assert_eq!(
            m.run_unproven(classified_upgrade("cora", "1.1.0", Some(wrong.clone()), 10)),
            Err(UPGRADE_CLASS_MISMATCH),
            "{wrong:?}"
        );
    }
    assert_eq!(m.state.apps["inst"].bundle, "b1", "a refusal moves nothing");
    // Green where the log shows nothing new; a class with a reason the log cannot judge
    // (a state version) is the proposer's to say.
    let mut green_move = SystemMachine {
        state: m.state.clone(),
        ..Default::default()
    };
    let _ok = green_move
        .run_unproven(classified_upgrade("cora", "1.0.1", Some(green()), 10))
        .unwrap();
    let rec = green_move.state.apps["inst"].last_upgrade.clone().unwrap();
    assert_eq!(rec.class, Some(calimero_bundle::update::Class::Green));
    assert!(rec.reasons.is_empty() && rec.approved_by.is_none());
    let _ok = m
        .run_unproven(classified_upgrade(
            "cora",
            "1.1.0",
            Some(red(&["https://telemetry.example/"])),
            10,
        ))
        .unwrap();
    let rec = m.state.apps["inst"].last_upgrade.clone().unwrap();
    assert_eq!(rec.class, Some(calimero_bundle::update::Class::Red));
    assert_eq!(rec.reasons, red(&["https://telemetry.example/"]).reasons);
    let shown = serde_json::to_value(&rec).unwrap();
    assert_eq!(shown["class"], "red", "{shown}");
    assert_eq!(shown["reasons"][0]["code"], "egressAdded", "{shown}");
    // An entry logged without a class replays as it applied, and records none.
    let mut old = classified();
    let _ok = old
        .run_unproven(upgrade_to("inst", "cora", "b2", Some(("p", "1.1.0"))))
        .unwrap();
    let rec = old.state.apps["inst"].last_upgrade.clone().unwrap();
    assert_eq!(rec.class, None);
    let shown = serde_json::to_value(&rec).unwrap();
    assert!(shown.get("class").is_none() && shown.get("approvedBy").is_none());
    // A raw bundle carries none.
    assert_eq!(
        classified()
            .run_unproven({
                let mut cmd = upgrade_to("inst", "cora", "raw", None);
                if let SystemCmd::UpgradeApp { class, .. } = &mut cmd {
                    *class = Some(green());
                }
                cmd
            })
            .unwrap_err(),
        UPGRADE_CLASS_MISMATCH
    );
}

/// An organization's update policy, and an app's own over it, are set by an owner or
/// admin with their passkey over exactly that policy; the hold is a day to a week; an app
/// follows its own, else its organization's, else the defaults (nothing on its own, one
/// approval). A state from before policies reads with none and writes as it did.
#[test]
fn an_update_policy_is_an_owners_or_admins_and_bounded() {
    use super::cluster_version::Feature;
    use super::{BAD_UPGRADE_POLICY, NOT_AN_UPGRADE_APPROVER};
    use crate::types::UpgradePolicy;
    let (mut m, mut alice, ..) = proven_cluster();
    let _prev = m.state.apps.insert(
        "oa".into(),
        serde_json::from_value(json!({
            "appId": "oa", "name": "oa", "group": 5, "bundle": "b1", "owner": "alice",
            "org": "org", "replicas": [1], "createdAt": 0
        }))
        .unwrap(),
    );
    let before = serde_json::to_value(&m.state.orgs["org"]).unwrap();
    assert!(before.get("upgradePolicy").is_none(), "{before}");
    let set = |app: Option<&str>, policy: Option<UpgradePolicy>, actor: &str, consent: Value| {
        SystemCmd::SetUpgradePolicy {
            org: "org".into(),
            app: app.map(str::to_owned),
            policy,
            actor: actor.into(),
            consent,
        }
    };
    let strict = UpgradePolicy {
        second_approver: true,
        ..UpgradePolicy::default()
    };
    assert_eq!(
        set(None, Some(strict), "alice", Value::Null).features(),
        [Feature::UpgradePolicies]
    );
    let (policy, source) = m.state.upgrade_policy_of(&m.state.apps["oa"]);
    assert_eq!((policy, source), (UpgradePolicy::default(), "default"));
    assert!(
        policy.auto_apply == crate::types::AutoApply::Never
            && !policy.second_approver
            && policy.hold_hours == 24
    );
    assert_eq!(
        m.run_unproven(set(None, Some(strict), "bob", Value::Null)),
        Err(NOT_AN_UPGRADE_APPROVER.into()),
        "a member"
    );
    needs_consent(
        &mut m,
        &mut alice,
        ("org:org", "__set_upgrade_policy"),
        &json!({ "app": null, "policy": { "autoApply": "never", "holdHours": 24, "secondApprover": true } }),
        &json!({ "app": null, "policy": null }),
        |c| set(None, Some(strict), "alice", c),
    );
    assert_eq!(
        m.state.upgrade_policy_of(&m.state.apps["oa"]),
        (strict, "org")
    );
    for hours in [0, 23, 169, u32::MAX] {
        let bad = UpgradePolicy {
            hold_hours: hours,
            ..UpgradePolicy::default()
        };
        assert_eq!(
            m.run_unproven(set(Some("oa"), Some(bad), "alice", Value::Null)),
            Err(BAD_UPGRADE_POLICY),
            "{hours}"
        );
    }
    let own = UpgradePolicy {
        auto_apply: crate::types::AutoApply::NoDestinations,
        hold_hours: 168,
        second_approver: false,
    };
    let answer = m
        .run_unproven(set(Some("oa"), Some(own), "alice", Value::Null))
        .unwrap();
    assert_eq!(answer["source"], "app", "{answer}");
    assert_eq!(answer["policy"]["holdHours"], 168, "{answer}");
    assert_eq!(m.state.upgrade_policy_of(&m.state.apps["oa"]), (own, "app"));
    assert!(
        m.run_unproven(set(Some("app"), Some(own), "alice", Value::Null))
            .is_err(),
        "an app of another organization"
    );
    let _ok = m
        .run_unproven(set(Some("oa"), None, "alice", Value::Null))
        .unwrap();
    assert_eq!(
        m.state.upgrade_policy_of(&m.state.apps["oa"]),
        (strict, "org"),
        "dropped: the organization's again"
    );
    // Unknown fields are refused; every field may be left out.
    assert!(serde_json::from_value::<UpgradePolicy>(json!({ "autoApply": "always" })).is_err());
    assert_eq!(
        serde_json::from_value::<UpgradePolicy>(json!({})).unwrap(),
        UpgradePolicy::default()
    );
    let restored: crate::types::Org = serde_json::from_value(before.clone()).unwrap();
    assert_eq!(serde_json::to_value(&restored).unwrap(), before);
}

/// With `secondApprover`, a red update (or one logged without a class) needs two distinct
/// owners or admins: the first logs a pending approval, the second applies it, and the
/// record names the first. A green one needs one. The same person twice, a member, an
/// approver no longer an admin, an approval of another release and one older than a week
/// let nothing through; an approval is spent once its app runs its target.
#[test]
fn a_red_update_needs_a_second_approver_where_the_policy_asks() {
    use super::{
        APPROVAL_PENDING, NOT_AN_UPGRADE_APPROVER, ONE_APPROVAL, SECOND_APPROVAL_REQUIRED,
    };
    use crate::types::{UpgradePolicy, MAX_APPROVAL_AGE_SECS};
    let telemetry = ["https://telemetry.example/"];
    let mut m = classified();
    let approve = |actor: &str, version: &str, class, at: u64| SystemCmd::ApproveUpgrade {
        app_id: "inst".into(),
        package: "p".into(),
        version: version.into(),
        targets: std::collections::BTreeMap::new(),
        class,
        actor: actor.into(),
        at,
        consent: Value::Null,
    };
    // Without the policy, one approval, and a first approval has nothing to wait for.
    assert_eq!(
        m.run_unproven(approve("cora", "1.1.0", red(&telemetry), 5)),
        Err(ONE_APPROVAL)
    );
    let _ok = m
        .run_unproven(SystemCmd::SetUpgradePolicy {
            org: "cust".into(),
            app: None,
            policy: Some(UpgradePolicy {
                second_approver: true,
                ..UpgradePolicy::default()
            }),
            actor: "eve".into(),
            consent: Value::Null,
        })
        .unwrap();
    // A green update still needs one.
    let mut green_move = SystemMachine {
        state: m.state.clone(),
        ..Default::default()
    };
    let _ok = green_move
        .run_unproven(classified_upgrade("cora", "1.0.1", Some(green()), 10))
        .unwrap();
    assert_eq!(
        m.run_unproven(approve("cora", "1.0.1", green(), 5)),
        Err(ONE_APPROVAL),
        "a green update needs no second"
    );
    // A red one, or one without a class, waits for a first approval by someone else.
    for class in [Some(red(&telemetry)), None] {
        assert_eq!(
            m.run_unproven(classified_upgrade("cora", "1.1.0", class, 10)),
            Err(SECOND_APPROVAL_REQUIRED)
        );
    }
    assert_eq!(
        m.run_unproven(approve("dan", "1.1.0", red(&telemetry), 5)),
        Err(NOT_AN_UPGRADE_APPROVER.into()),
        "a member"
    );
    assert_eq!(
        m.run_unproven(approve("cora", "1.1.0", green(), 5)),
        Err(ONE_APPROVAL),
        "approved as green"
    );
    assert!(
        m.run(approve("cora", "1.1.0", red(&telemetry), 5)).is_err(),
        "without cora's passkey"
    );
    let pending = m
        .run_unproven(approve("cora", "1.1.0", red(&telemetry), 5))
        .unwrap();
    assert_eq!(pending["by"], "cora", "{pending}");
    assert_eq!(
        m.run_unproven(approve("eve", "1.1.0", red(&telemetry), 6)),
        Err(APPROVAL_PENDING),
        "the second approver applies it instead"
    );
    // Not the same person twice, nor a week and a second later.
    assert_eq!(
        m.run_unproven(classified_upgrade(
            "cora",
            "1.1.0",
            Some(red(&telemetry)),
            10
        )),
        Err(SECOND_APPROVAL_REQUIRED)
    );
    let late = 5 + MAX_APPROVAL_AGE_SECS + 1;
    assert_eq!(
        SystemMachine {
            state: m.state.clone(),
            ..Default::default()
        }
        .run_unproven(classified_upgrade(
            "eve",
            "1.1.0",
            Some(red(&telemetry)),
            late
        )),
        Err(SECOND_APPROVAL_REQUIRED)
    );
    // Not once the first approver is no longer an admin.
    let mut demoted = SystemMachine {
        state: m.state.clone(),
        ..Default::default()
    };
    if let Some(o) = demoted.state.orgs.get_mut("cust") {
        let _prev = o
            .members
            .insert("cora".into(), crate::types::OrgRole::Member);
        let _prev = o.members.insert("pam".into(), crate::types::OrgRole::Owner);
    }
    assert_eq!(
        demoted.run_unproven(classified_upgrade(
            "eve",
            "1.1.0",
            Some(red(&telemetry)),
            10
        )),
        Err(SECOND_APPROVAL_REQUIRED)
    );
    // The second applies it; the record names the first, and the approval is spent.
    let _ok = m
        .run_unproven(classified_upgrade(
            "eve",
            "1.1.0",
            Some(red(&telemetry)),
            10,
        ))
        .unwrap();
    let rec = m.state.apps["inst"].last_upgrade.clone().unwrap();
    assert_eq!(
        (rec.by.as_str(), rec.approved_by.as_deref()),
        ("eve", Some("cora"))
    );
    assert!(m.state.orgs["cust"].pending_upgrades.is_empty());
}

/// A first approval names only a release its approver may use, from the logged version that
/// brings private releases: one private to another organization is refused as a missing
/// one, and an entry logged below it replays as it applied.
#[test]
fn a_first_approval_of_a_release_its_approver_may_not_use_is_refused() {
    use super::cluster_version::Feature;
    use crate::types::UpgradePolicy;
    let mut m = classified();
    let _ok = m
        .run_unproven(SystemCmd::SetUpgradePolicy {
            org: "cust".into(),
            app: None,
            policy: Some(UpgradePolicy {
                second_approver: true,
                ..UpgradePolicy::default()
            }),
            actor: "eve".into(),
            consent: Value::Null,
        })
        .unwrap();
    if let Some(v) = m
        .state
        .packages
        .get_mut("p")
        .and_then(|p| p.versions.get_mut("1.1.0"))
    {
        v.visibility = Some(super::Visibility::Private);
        v.orgs = std::collections::BTreeSet::from(["elsewhere".to_owned()]);
    }
    let approve = SystemCmd::ApproveUpgrade {
        app_id: "inst".into(),
        package: "p".into(),
        version: "1.1.0".into(),
        targets: std::collections::BTreeMap::new(),
        class: red(&["https://telemetry.example/"]),
        actor: "cora".into(),
        at: 5,
        consent: Value::Null,
    };
    let mut old = SystemMachine {
        state: m.state.clone(),
        ..Default::default()
    };
    assert!(
        old.run_unproven(approve.clone()).is_ok(),
        "replayed as it applied"
    );
    m.state.cluster_version = Some(Feature::PrivateReleases.since().to_owned());
    assert_eq!(
        m.run_unproven(approve),
        Err("that release is not installed".into())
    );
}

// ------------------------------------------------------- updates on their own, undo

/// C9's fixture: `publishing`'s customer `cust` (cora its owner, eve an admin, dan a
/// member) with `inst` on `p@1.0.0` (bundle `b1`), and nodes 1 and 2. No release sends
/// data out but `1.1.0`: `1.0.1` (`b3`, installed at 1000) and `1.0.2` (`b3` again) are
/// green, `1.1.0` (`b2`) adds a telemetry host.
fn auto_fixture() -> SystemMachine {
    let (_key, signer) = publisher_key(43);
    let mut m = publishing(&signer, "owner");
    let _ok = m.run_unproven(account("eve")).unwrap();
    if let Some(o) = m.state.orgs.get_mut("cust") {
        let _prev = o.members.insert("eve".into(), crate::types::OrgRole::Admin);
    }
    register(&mut m, 1, "replica");
    register(&mut m, 2, "replica");
    let _prev = m.state.bundles.insert(
        "b3".into(),
        serde_json::from_value(json!({
            "hash": "b3", "size": 1, "uploadedBy": "pam",
            "package": { "package": "p", "version": "1.0.1" }
        }))
        .unwrap(),
    );

    if let Some(p) = m.state.packages.get_mut("p") {
        for (version, mpk, bundle, at, outbound) in [
            ("1.0.0", "00", "b1", 0, json!([])),
            ("1.0.1", "02", "b3", 1000, json!([])),
            ("1.0.2", "03", "b3", 1000, json!([])),
            (
                "1.1.0",
                "01",
                "b2",
                1000,
                json!(["https://telemetry.example/"]),
            ),
        ] {
            let _prev = p.versions.insert(
                version.into(),
                serde_json::from_value(json!({
                    "version": version, "mpk": mpk, "mpkSize": 1, "source": "upload",
                    "services": { "": bundle }, "installedBy": "pam", "installedAt": at,
                    "outbound": outbound
                }))
                .unwrap(),
            );
        }
    }
    instance_of(
        &mut m,
        "inst",
        "b1",
        json!({ "package": "p", "version": "1.0.0" }),
    );
    m
}

/// The restore point node `node` took of group 3 at index `index`.
fn taken_on(node: u64, index: u64) -> crate::types::TakenRestorePoint {
    crate::types::TakenRestorePoint {
        file: Some(crate::types::RestorePointFile {
            node,
            group: 3,
            source: crate::types::RestoreSource {
                name: format!("{index}.3.s.snap"),
                sha256: "ab".repeat(32),
                index,
            },
        }),
    }
}

/// `inst`'s update to `version` (its bundle) applying on its own at `at`.
fn auto_upgrade(
    version: &str,
    bundle: &str,
    class: Option<crate::types::UpgradeClass>,
    at: u64,
    taken: Option<crate::types::TakenRestorePoint>,
) -> SystemCmd {
    let mut cmd = upgrade_to("inst", super::AUTO_UPDATE, bundle, Some(("p", version)));
    if let SystemCmd::UpgradeApp {
        class: c,
        owner_approves,
        at: when,
        auto,
        restore_point,
        ..
    } = &mut cmd
    {
        *c = class;
        *owner_approves = true;
        *when = at;
        *auto = true;
        *restore_point = taken;
    }
    cmd
}

/// The policy `cust` sets for every app: green updates of apps that send no data out
/// apply on their own after `hold` hours.
fn automatic(m: &mut SystemMachine, hold: u32) {
    let _ok = m
        .run_unproven(SystemCmd::SetUpgradePolicy {
            org: "cust".into(),
            app: None,
            policy: Some(crate::types::UpgradePolicy {
                auto_apply: crate::types::AutoApply::NoDestinations,
                hold_hours: hold,
                second_approver: false,
            }),
            actor: "eve".into(),
            consent: Value::Null,
        })
        .unwrap();
}

/// A green update applies on its own only under a `noDestinations` policy, once its hold
/// has passed since the release was logged (judged on the upgrade's own `at`), with a
/// restore point, to a newer release, for an app whose unit sends no data out before or
/// after. It needs no person's consent; it is recorded as the policy's, and its restore
/// point becomes the app's one undo. A red update, a destination, an older release, a
/// member alone or an update proposed as anyone else never applies this way.
#[test]
fn a_green_update_applies_on_its_own_after_its_hold_under_the_policy() {
    use super::cluster_version::Feature;
    use super::{AUTO_UPDATE, AUTO_UPDATE_REFUSED, HOLD_NOT_OVER, RESTORE_POINT_REQUIRED};
    let hold_over = 1000 + 24 * 3600;
    let mut m = auto_fixture();
    let cmd = auto_upgrade(
        "1.0.1",
        "b3",
        Some(green()),
        hold_over,
        Some(taken_on(1, 7)),
    );
    assert_eq!(
        cmd.features(),
        [
            Feature::OwnerApprovedUpgrades,
            Feature::UpgradeClasses,
            Feature::AutoUpdates,
            Feature::RestorePoints
        ]
    );
    // No policy: never on its own.
    assert_eq!(m.run(cmd.clone()), Err(AUTO_UPDATE_REFUSED));
    assert!(m.state.auto_update_candidates(hold_over).is_empty());
    automatic(&mut m, 24);
    // Not before the hold, judged on the logged times.
    assert_eq!(
        m.run(auto_upgrade(
            "1.0.1",
            "b3",
            Some(green()),
            hold_over - 1,
            Some(taken_on(1, 7))
        )),
        Err(HOLD_NOT_OVER)
    );
    assert!(m.state.auto_update_candidates(hold_over - 1).is_empty());
    // Not without a restore point of the group it runs on.
    for taken in [None, Some(crate::types::TakenRestorePoint::default())] {
        assert_eq!(
            m.run(auto_upgrade("1.0.1", "b3", Some(green()), hold_over, taken)),
            Err(RESTORE_POINT_REQUIRED)
        );
    }
    // Not red or unclassified, not with a destination, not back to an older release.
    for (version, bundle, class) in [
        ("1.0.1", "b3", None),
        ("1.0.1", "b3", Some(red(&["https://x.example/"]))),
        ("1.1.0", "b2", Some(red(&["https://telemetry.example/"]))),
        ("1.0.0", "b1", Some(green())),
    ] {
        assert!(
            m.run(auto_upgrade(
                version,
                bundle,
                class.clone(),
                hold_over,
                Some(taken_on(1, 7))
            ))
            .is_err(),
            "{version} {class:?}"
        );
    }
    // Not proposed as a person, nor as the policy without the marker.
    let mut as_cora = cmd.clone();
    if let SystemCmd::UpgradeApp { actor, .. } = &mut as_cora {
        *actor = "cora".into();
    }
    assert_eq!(m.run(as_cora), Err(AUTO_UPDATE_REFUSED));
    let mut unmarked = cmd.clone();
    if let SystemCmd::UpgradeApp { auto, .. } = &mut unmarked {
        *auto = false;
    }
    assert!(
        m.run(unmarked).is_err(),
        "the policy's actor has no passkey"
    );
    assert_eq!(m.state.apps["inst"].bundle, "b1", "a refusal moves nothing");
    // The leader finds it once the hold passed: the newest release, green and quiet.
    let found = m.state.auto_update_candidates(hold_over);
    assert_eq!(found.len(), 1);
    assert_eq!(
        (found[0].0.app_id.as_str(), found[0].1[0].version.as_str()),
        ("inst", "1.0.2"),
        "the newest release of its package first"
    );
    // Then each older one the leader falls back to should it classify the newest red;
    // never `1.1.0`, which the log shows sends data out.
    let order: Vec<&str> = found[0].1.iter().map(|r| r.version.as_str()).collect();
    assert_eq!(order, ["1.0.2", "1.0.1"]);
    let _ok = m.run(cmd).unwrap();
    let a = &m.state.apps["inst"];
    assert_eq!(
        (
            a.bundle.as_str(),
            a.release.as_ref().unwrap().version.as_str()
        ),
        ("b3", "1.0.1")
    );
    let rec = a.last_upgrade.clone().unwrap();
    assert!(rec.auto && rec.by == AUTO_UPDATE && rec.ok);
    let shown = serde_json::to_value(&rec).unwrap();
    assert_eq!(shown["auto"], true, "{shown}");
    let p = a.restore_point.clone().unwrap();
    assert_eq!(
        (p.from_bundle.as_str(), p.to_bundle.as_str(), p.at),
        ("b1", "b3", hold_over)
    );
    assert_eq!(p.from_release.unwrap().version, "1.0.0");
    assert_eq!(p.file, taken_on(1, 7).file);
    // A unit's member never moves alone on its own.
    let mut parent = m.state.apps["inst"].clone();
    parent.app_id = "parent".into();
    let _prev = parent.links.insert("store".into(), "inst".into());
    let _prev = m.state.apps.insert("parent".into(), parent);
    assert_eq!(
        m.run(auto_upgrade(
            "1.0.2",
            "b3",
            Some(green()),
            hold_over,
            Some(crate::types::TakenRestorePoint::default())
        )),
        Err(AUTO_UPDATE_REFUSED)
    );
}

/// An update applies on its own only to a release the app's owner may use: a stranger to
/// the organization a private release was installed for never moves onto it, from a public
/// one or any other, from the logged version that brings private releases; an entry logged
/// below it replays as it applied.
#[test]
fn an_update_on_its_own_moves_only_to_a_release_its_owner_may_use() {
    use super::cluster_version::Feature;
    use super::{Visibility, AUTO_UPDATE_REFUSED};
    let hold_over = 1000 + 24 * 3600;
    let mut m = auto_fixture();
    automatic(&mut m, 24);
    let p = m.state.packages.get_mut("p").unwrap();
    p.versions.get_mut("1.0.0").unwrap().visibility = Some(Visibility::Public);
    for v in ["1.0.1", "1.0.2"] {
        let v = p.versions.get_mut(v).unwrap();
        v.visibility = Some(Visibility::Private);
        v.orgs = BTreeSet::from(["pub".to_owned()]);
    }
    let cmd = auto_upgrade(
        "1.0.1",
        "b3",
        Some(green()),
        hold_over,
        Some(taken_on(1, 7)),
    );
    let mut old = SystemMachine {
        state: m.state.clone(),
        ..Default::default()
    };
    let _ok = old.run(cmd.clone()).expect("replayed as it applied");

    m.state.cluster_version = Some(Feature::PrivateReleases.since().to_owned());
    assert!(m.state.auto_update_candidates(hold_over).is_empty());
    assert_eq!(m.run(cmd.clone()), Err(AUTO_UPDATE_REFUSED.to_owned()));
    assert_eq!(m.state.apps["inst"].bundle, "b1");
    // Installed for the app's own organization too, it is its owner's to move onto.
    let _new = m
        .state
        .packages
        .get_mut("p")
        .unwrap()
        .versions
        .get_mut("1.0.1")
        .unwrap()
        .orgs
        .insert("cust".to_owned());
    let found = m.state.auto_update_candidates(hold_over);
    assert_eq!(found[0].1.len(), 1, "{found:?}");
    assert_eq!(found[0].1[0].version, "1.0.1");
    let _ok = m.run(cmd).unwrap();
}

/// For a week after an upgrade, an owner or admin undoes it with their passkey: the app
/// goes back to the release it ran, into a new group the restore point's node seeds
/// (whatever it wrote since is lost), the one move to an older release; the restore
/// point is spent once that restore finishes. A member, a stale or late undo, other apps
/// or other seeds than the restore points name are refused; an upgrade without a
/// restore point leaves none, and an update an undo left never applies on its own again.
#[test]
fn an_upgrade_is_undone_within_a_week_back_to_its_restore_point() {
    use super::{
        NOT_AN_UPGRADE_APPROVER, NO_RESTORE_POINT, UNDO_NOT_AS_NAMED, UNDO_STALE,
        UNDO_WINDOW_PASSED,
    };
    use crate::types::UNDO_WINDOW_SECS;
    let hold_over = 1000 + 24 * 3600;
    let mut m = auto_fixture();
    automatic(&mut m, 24);
    let _ok = m
        .run(auto_upgrade(
            "1.0.1",
            "b3",
            Some(green()),
            hold_over,
            Some(taken_on(2, 7)),
        ))
        .unwrap();
    let undo = |actor: &str, apps: Value, at: u64| SystemCmd::UndoUpgrade {
        app_id: "inst".into(),
        apps: serde_json::from_value(apps).unwrap(),
        actor: actor.into(),
        at,
        consent: Value::Null,
        release_installed: false,
    };
    let within = hold_over + UNDO_WINDOW_SECS;
    assert_eq!(
        undo("cora", json!({ "inst": [2, 1] }), within).features(),
        [super::cluster_version::Feature::RestorePoints]
    );
    assert_eq!(
        m.run_unproven(undo("dan", json!({ "inst": [2, 1] }), within)),
        Err(NOT_AN_UPGRADE_APPROVER.into()),
        "a member"
    );
    for (apps, why) in [
        (json!({}), "no app"),
        (
            json!({ "inst": [1, 2] }),
            "seeded by a node without the restore point",
        ),
        (json!({ "inst": [] }), "no new group"),
        (json!({ "inst": [2, 2] }), "a node twice"),
        (json!({ "inst": [2, 9] }), "an unregistered node"),
        (json!({ "inst": [2], "rawapp": [] }), "another app"),
    ] {
        assert_eq!(
            m.run_unproven(undo("cora", apps, within)),
            Err(UNDO_NOT_AS_NAMED),
            "{why}"
        );
    }
    assert_eq!(
        m.run_unproven(undo("cora", json!({ "inst": [2, 1] }), within + 1)),
        Err(UNDO_WINDOW_PASSED)
    );
    assert!(
        m.run(undo("cora", json!({ "inst": [2, 1] }), within))
            .is_err(),
        "without the owner's passkey"
    );
    let args = m.state.undo_args(["inst".to_owned()].iter());
    assert_eq!(args["apps"]["inst"]["to"]["version"], "1.0.0", "{args}");
    assert_eq!(args["apps"]["inst"]["from"]["version"], "1.0.1", "{args}");
    assert_eq!(
        args["apps"]["inst"]["restorePoint"],
        "ab".repeat(32),
        "{args}"
    );
    let before = m.state.apps["inst"].group;
    let out = m
        .run_unproven(undo("eve", json!({ "inst": [2, 1] }), within))
        .unwrap();
    let a = m.state.apps["inst"].clone();
    assert_eq!(out["groups"]["inst"], a.group, "{out}");
    assert_ne!(a.group, before, "a new group");
    assert_eq!(
        (
            a.bundle.as_str(),
            a.release.as_ref().unwrap().version.as_str()
        ),
        ("b1", "1.0.0"),
        "the one move to an older release"
    );
    assert_eq!(a.replicas, [2, 1]);
    let restore = a.restore.clone().unwrap();
    assert_eq!(restore.seed, Some(2));
    assert_eq!(restore.source, taken_on(2, 7).file.map(|f| f.source));
    let rec = a.last_upgrade.clone().unwrap();
    assert!(rec.undo && rec.by == "eve");
    assert_eq!(rec.from_release.unwrap().version, "1.0.1");
    assert_eq!(rec.to_release.unwrap().version, "1.0.0");
    // A seed that stopped may be sent again until the restore finishes.
    let again = m
        .run_unproven(undo("cora", json!({ "inst": [2, 1] }), within + 100))
        .unwrap();
    let group = m.state.apps["inst"].group;
    assert_eq!(again["groups"]["inst"], group);
    let _ok = m
        .run_unproven(SystemCmd::FinishRestore {
            app_id: "inst".into(),
            group,
        })
        .unwrap();
    assert!(m.state.apps["inst"].restore_point.is_none(), "spent");
    assert_eq!(
        m.run_unproven(undo("cora", json!({ "inst": [2, 1] }), within)),
        Err(NO_RESTORE_POINT)
    );
    // The release it undid never applies on its own again; a newer one may.
    let later = hold_over + 10;
    let found = m.state.auto_update_candidates(later);
    assert_eq!(found[0].1[0].version, "1.0.2");
    if let Some(p) = m.state.packages.get_mut("p") {
        let _gone = p.versions.remove("1.0.2");
    }
    assert!(m.state.auto_update_candidates(later).is_empty());

    // An upgrade that runs nothing on the group is undone in the directory alone.
    let mut m = auto_fixture();
    let manual = |version: &str, taken| {
        let mut cmd = classified_upgrade("cora", version, Some(green()), 50);
        if let SystemCmd::UpgradeApp {
            bundle,
            restore_point,
            ..
        } = &mut cmd
        {
            *bundle = "b3".into();
            *restore_point = taken;
        }
        cmd
    };
    let _ok = m
        .run_unproven(manual("1.0.1", Some(taken_on(1, 3))))
        .unwrap();
    let _ok = m
        .run_unproven(manual(
            "1.0.2",
            Some(crate::types::TakenRestorePoint::default()),
        ))
        .unwrap();
    assert_eq!(
        m.run_unproven(undo("cora", json!({ "inst": [1] }), 60)),
        Err(UNDO_NOT_AS_NAMED)
    );
    let group = m.state.apps["inst"].group;
    let _ok = m
        .run_unproven(undo("cora", json!({ "inst": [] }), 60))
        .unwrap();
    let a = &m.state.apps["inst"];
    assert_eq!(
        (a.group, a.release.as_ref().unwrap().version.as_str()),
        (group, "1.0.1")
    );
    assert!(a.restore.is_none() && a.restore_point.is_none());
    // Once the app moved on, an older restore point is stale; without one, none is kept.
    let mut m = auto_fixture();
    let _ok = m
        .run_unproven(manual("1.0.1", Some(taken_on(1, 3))))
        .unwrap();
    if let Some(a) = m.state.apps.get_mut("inst") {
        a.bundle = "b2".into();
    }
    assert_eq!(
        m.run_unproven(undo("cora", json!({ "inst": [1] }), 60)),
        Err(UNDO_STALE)
    );
    if let Some(a) = m.state.apps.get_mut("inst") {
        a.bundle = "b3".into();
    }
    let _ok = m.run_unproven(manual("1.0.2", None)).unwrap();
    assert!(m.state.apps["inst"].restore_point.is_none());
    // A state from before restore points reads with none and writes as it did.
    let shown = serde_json::to_value(&m.state.apps["rawapp"]).unwrap();
    assert!(shown.get("restorePoint").is_none(), "{shown}");
}

/// An undo goes back only to a release that is still installed: a developer who
/// uninstalls a release it no longer offers takes it from the customers' undo too, even
/// where another release ships the same WASM and the bundle stays. An entry logged
/// before the rule replays as it applied.
#[test]
fn an_undo_never_returns_to_a_release_that_is_no_longer_installed() {
    use super::cluster_version::Feature;
    use super::UNDO_RELEASE_GONE;
    let moved_to_the_shared_wasm = || {
        let mut m = auto_fixture();
        let manual = |version: &str, taken| {
            let mut cmd = classified_upgrade("cora", version, Some(green()), 50);
            if let SystemCmd::UpgradeApp {
                bundle,
                restore_point,
                ..
            } = &mut cmd
            {
                *bundle = "b3".into();
                *restore_point = taken;
            }
            cmd
        };
        let _ok = m
            .run_unproven(manual("1.0.1", Some(taken_on(1, 3))))
            .unwrap();
        let _ok = m
            .run_unproven(manual(
                "1.0.2",
                Some(crate::types::TakenRestorePoint::default()),
            ))
            .unwrap();
        let _ok = m
            .run_unproven(SystemCmd::UninstallPackage {
                package: "p".into(),
                version: Some("1.0.1".into()),
                actor: "pam".into(),
            })
            .unwrap();
        assert!(m.state.bundles.contains_key("b3"), "1.0.2 still ships it");
        m
    };
    let undo = |release_installed: bool| SystemCmd::UndoUpgrade {
        app_id: "inst".into(),
        apps: serde_json::from_value(json!({ "inst": [] })).unwrap(),
        actor: "cora".into(),
        at: 60,
        consent: Value::Null,
        release_installed,
    };
    assert_eq!(
        undo(true).features(),
        [Feature::RestorePoints, Feature::UndoToInstalledRelease]
    );
    assert_eq!(undo(false).features(), [Feature::RestorePoints]);

    let mut m = moved_to_the_shared_wasm();
    assert_eq!(m.run_unproven(undo(true)), Err(UNDO_RELEASE_GONE));
    let a = &m.state.apps["inst"];
    assert_eq!(
        a.release.as_ref().unwrap().version,
        "1.0.2",
        "nothing moved"
    );
    assert!(a.restore_point.is_some(), "the restore point is kept");
    // The logged cluster version brings the rule to an entry without the flag.
    m.state.cluster_version = Some(Feature::UndoToInstalledRelease.since().to_owned());
    assert_eq!(m.run_unproven(undo(false)), Err(UNDO_RELEASE_GONE));
    // An entry logged before it, with no flag, applied.
    let mut m = moved_to_the_shared_wasm();
    let _ok = m.run_unproven(undo(false)).unwrap();
    assert_eq!(
        m.state.apps["inst"].release.as_ref().unwrap().version,
        "1.0.1"
    );
    // A seed sent again after the undo moved the app is not refused for a release uninstalled
    // since: the app already runs it and waits for its restore.
    let mut m = auto_fixture();
    for (version, taken) in [("1.0.1", taken_on(1, 3)), ("1.0.2", taken_on(2, 7))] {
        let mut up = classified_upgrade("cora", version, Some(green()), 50);
        if let SystemCmd::UpgradeApp {
            bundle,
            restore_point,
            ..
        } = &mut up
        {
            *bundle = "b3".into();
            *restore_point = Some(taken);
        }
        let _ok = m.run_unproven(up).unwrap();
    }
    let seeded = |release_installed| SystemCmd::UndoUpgrade {
        app_id: "inst".into(),
        apps: serde_json::from_value(json!({ "inst": [2, 1] })).unwrap(),
        actor: "cora".into(),
        at: 60,
        consent: Value::Null,
        release_installed,
    };
    let _ok = m.run_unproven(seeded(true)).unwrap();
    let _ok = m
        .run_unproven(SystemCmd::UninstallPackage {
            package: "p".into(),
            version: Some("1.0.1".into()),
            actor: "pam".into(),
        })
        .unwrap();
    let _ok = m.run_unproven(seeded(true)).unwrap();
    // Installed, the release is undone to as ever, flagged or not.
    let mut m = auto_fixture();
    let mut up = classified_upgrade("cora", "1.0.1", Some(green()), 50);
    if let SystemCmd::UpgradeApp {
        bundle,
        restore_point,
        ..
    } = &mut up
    {
        *bundle = "b3".into();
        *restore_point = Some(crate::types::TakenRestorePoint::default());
    }
    let _ok = m.run_unproven(up).unwrap();
    let _ok = m.run_unproven(undo(true)).unwrap();
    assert_eq!(
        m.state.apps["inst"].release.as_ref().unwrap().version,
        "1.0.0"
    );
}

/// An instance of a deployment applies its offer on its own once its hold has passed
/// since the offer of exactly that release was logged (`addedAt`), not since its install;
/// the leader finds it only then, and only the release its deployment offers.
#[test]
fn a_deployment_instances_hold_runs_from_its_offer() {
    use super::{Deployer, HOLD_NOT_OVER};
    let mut m = auto_fixture();
    automatic(&mut m, 24);
    let _ok = m
        .run_unproven(create_deployment(
            "d",
            Deployer::Account("pam".into()),
            "pam",
        ))
        .unwrap();
    // 1.0.1 was installed at 1000; the deployment offers it at 2000.
    let mut offer = offer_app("d", "notes", "1.0.1", "pam");
    if let SystemCmd::OfferApp { at, .. } = &mut offer {
        *at = 2000;
    }
    let _ok = m.run_unproven(offer).unwrap();
    if let Some(a) = m.state.apps.get_mut("inst") {
        a.deployment = Some("d".into());
    }
    let day = 24 * 3600;
    let up = |at: u64| {
        let mut cmd = auto_upgrade("1.0.1", "b3", Some(green()), at, Some(taken_on(1, 4)));
        if let SystemCmd::UpgradeApp { in_deployment, .. } = &mut cmd {
            *in_deployment = true;
        }
        cmd
    };
    // A day after the install, but not after the offer: it waits.
    assert!(m.state.auto_update_candidates(1000 + day).is_empty());
    assert_eq!(m.run(up(1000 + day)), Err(HOLD_NOT_OVER));
    // A day after the offer: found, with the offered release alone, and applied.
    let found = m.state.auto_update_candidates(2000 + day);
    let versions: Vec<&str> = found[0].1.iter().map(|r| r.version.as_str()).collect();
    assert_eq!(
        (found[0].0.app_id.as_str(), versions),
        ("inst", vec!["1.0.1"])
    );
    let _ok = m.run(up(2000 + day)).unwrap();
    assert_eq!(m.state.apps["inst"].bundle, "b3");
}

/// An instance of a deployment updates on its own only from an offer that still serves: once
/// nobody who acts for the deployment's owner may use the offered release, the leader finds
/// nothing to apply, from the logged version that brings private releases.
#[test]
fn a_deployment_instance_updates_on_its_own_only_from_an_offer_that_serves() {
    use super::{cluster_version::Feature, Deployer};
    let mut m = auto_fixture();
    automatic(&mut m, 24);
    let _ok = m
        .run_unproven(create_deployment(
            "d",
            Deployer::Account("pam".into()),
            "pam",
        ))
        .unwrap();
    let _ok = m
        .run_unproven(offer_app("d", "notes", "1.0.1", "pam"))
        .unwrap();
    if let Some(a) = m.state.apps.get_mut("inst") {
        a.deployment = Some("d".into());
    }
    if let Some(v) = m
        .state
        .packages
        .get_mut("p")
        .and_then(|p| p.versions.get_mut("1.0.1"))
    {
        v.visibility = Some(super::Visibility::Private);
        v.orgs = std::collections::BTreeSet::from(["elsewhere".to_owned()]);
    }
    let later = 10 * 24 * 3600;
    assert_eq!(m.state.auto_update_candidates(later).len(), 1, "as before");
    m.state.cluster_version = Some(Feature::PrivateReleases.since().to_owned());
    assert!(m.state.auto_update_candidates(later).is_empty());
}

/// A move the log cannot rebuild (the release the app runs, or a member's, no longer
/// installed: allowed while another release ships its WASM) is never applied as green:
/// the log cannot confirm it, so a claimed green is refused and a red one, gated as red,
/// is the proposer's to say.
#[test]
fn a_move_the_log_cannot_rebuild_is_never_applied_as_green() {
    use super::UPGRADE_CLASS_MISMATCH;
    let telemetry = ["https://telemetry.example/"];
    // `inst` runs p@1.0.0, uninstalled while p@1.0.1 still ships b1.
    let mut m = classified();
    let _gone = m
        .state
        .packages
        .get_mut("p")
        .and_then(|p| p.versions.remove("1.0.0"));
    assert_eq!(
        m.run_unproven(classified_upgrade("cora", "1.1.0", Some(green()), 10)),
        Err(UPGRADE_CLASS_MISMATCH),
        "a destination added, logged as green"
    );
    assert_eq!(m.state.apps["inst"].bundle, "b1", "a refusal moves nothing");
    let _ok = m
        .run_unproven(classified_upgrade(
            "cora",
            "1.1.0",
            Some(red(&telemetry)),
            10,
        ))
        .unwrap();
    // A member whose release is no longer installed hides nothing either.
    let mut m = classified();
    instance_of(
        &mut m,
        "mem",
        "b1",
        json!({ "package": "p", "version": "1.0.1" }),
    );
    if let Some(a) = m.state.apps.get_mut("inst") {
        let _prev = a.links.insert("store".into(), "mem".into());
    }
    let _gone = m
        .state
        .packages
        .get_mut("p")
        .and_then(|p| p.versions.remove("1.0.1"));
    assert_eq!(
        m.run_unproven(classified_upgrade("cora", "1.1.0", Some(green()), 10)),
        Err(UPGRADE_CLASS_MISMATCH),
        "a member the log cannot rebuild"
    );
}

/// A move the log cannot classify (the release the app runs uninstalled while another
/// ships its WASM) still gets its first approval under `secondApprover`: red, its reason
/// `unclassified` naming the release the log lacks, as `SystemState::unclassified` says
/// from the log alone. A second approver applies it with that approval. An `unclassified`
/// reason naming anything else, or on a move the log rebuilds, is refused.
#[test]
fn an_unclassified_move_is_approved_first_as_red_naming_what_the_log_lacks() {
    use super::UPGRADE_CLASS_MISMATCH;
    use crate::types::{UpgradeClass, UpgradePolicy};
    use calimero_bundle::update::{Class, Reason};
    let unclassified = |version: &str| UpgradeClass {
        class: Class::Red,
        reasons: vec![Reason::Unclassified {
            member: None,
            package: "p".into(),
            version: version.into(),
        }],
    };
    let approve = |actor: &str, class, at: u64| SystemCmd::ApproveUpgrade {
        app_id: "inst".into(),
        package: "p".into(),
        version: "1.1.0".into(),
        targets: std::collections::BTreeMap::new(),
        class,
        actor: actor.into(),
        at,
        consent: Value::Null,
    };
    let mut m = classified();
    let _ok = m
        .run_unproven(SystemCmd::SetUpgradePolicy {
            org: "cust".into(),
            app: None,
            policy: Some(UpgradePolicy {
                second_approver: true,
                ..UpgradePolicy::default()
            }),
            actor: "eve".into(),
            consent: Value::Null,
        })
        .unwrap();
    let empty = std::collections::BTreeMap::new();
    // While the log rebuilds the move, nothing is unclassified, and no reason says so.
    let inst = m.state.apps["inst"].clone();
    assert_eq!(m.state.unclassified(&inst, ("p", "1.1.0"), &empty), None);
    assert_eq!(
        m.run_unproven(approve("cora", unclassified("1.0.0"), 5)),
        Err(UPGRADE_CLASS_MISMATCH)
    );
    // `inst` runs p@1.0.0, uninstalled while p@1.0.1 still ships b1.
    let _gone = m
        .state
        .packages
        .get_mut("p")
        .and_then(|p| p.versions.remove("1.0.0"));
    assert_eq!(
        m.state.unclassified(&inst, ("p", "1.1.0"), &empty),
        Some(unclassified("1.0.0"))
    );
    assert_eq!(
        m.run_unproven(approve("cora", unclassified("1.0.1"), 5)),
        Err(UPGRADE_CLASS_MISMATCH),
        "a release the log holds"
    );
    let pending = m
        .run_unproven(approve("cora", unclassified("1.0.0"), 5))
        .unwrap();
    assert_eq!(pending["class"]["class"], "red", "{pending}");
    assert_eq!(
        pending["class"]["reasons"],
        json!([{ "code": "unclassified", "member": null, "package": "p", "version": "1.0.0" }]),
        "{pending}"
    );
    // The same person twice is refused; another admin applies it, recorded red.
    assert!(m
        .run_unproven(classified_upgrade(
            "cora",
            "1.1.0",
            Some(unclassified("1.0.0")),
            10
        ))
        .is_err());
    let _ok = m
        .run_unproven(classified_upgrade(
            "eve",
            "1.1.0",
            Some(unclassified("1.0.0")),
            10,
        ))
        .unwrap();
    let rec = m.state.apps["inst"].last_upgrade.clone().unwrap();
    assert_eq!(
        (rec.by.as_str(), rec.approved_by.as_deref()),
        ("eve", Some("cora"))
    );
    assert_eq!(rec.class, Some(Class::Red));
}

/// A unit's run is judged once, as a whole, under its instance's policy: the instance's
/// `UpgradeApp` carries the unit's class and is judged over the members' targets (a unit
/// red only through a member is never logged green), and each member it moves takes the
/// instance's judgement in that run, so a member's own policy cannot stop the unit half
/// way. A member upgraded alone keeps its own policy.
#[test]
fn a_units_run_is_judged_once_under_its_instances_policy() {
    use super::{SECOND_APPROVAL_REQUIRED, UPGRADE_CLASS_MISMATCH};
    use crate::types::{RunTarget, UpgradePolicy, UpgradeRun};
    let telemetry = ["https://telemetry.example/"];
    let mut m = classified();
    // `inst` on p@1.0.0 created `mem`, also on p@1.0.0; the unit moves `inst` to the green
    // p@1.0.1 and `mem` to p@1.1.0, which adds a telemetry host: red only through `mem`.
    instance_of(
        &mut m,
        "mem",
        "b1",
        json!({ "package": "p", "version": "1.0.0" }),
    );
    if let Some(a) = m.state.apps.get_mut("inst") {
        let _prev = a.links.insert("store".into(), "mem".into());
    }
    let targets: std::collections::BTreeMap<String, RunTarget> = std::iter::once((
        "mem".to_owned(),
        RunTarget {
            package: "p".into(),
            version: "1.1.0".into(),
        },
    ))
    .collect();
    let run = UpgradeRun {
        run_id: "u".into(),
        package: "p".into(),
        version: "1.0.1".into(),
        team: None,
        cascade: false,
        by: "cora".into(),
        started_at: 9,
        apps: vec!["inst".into(), "mem".into()],
        done: Vec::new(),
        failed: std::collections::BTreeMap::new(),
        finished: false,
        targets,
    };
    if let Some(o) = m.state.orgs.get_mut("cust") {
        let _prev = o.upgrade_runs.insert("u".into(), run);
        // Only the member asks for a second approver.
        let _prev = o.app_upgrade_policies.insert(
            "mem".into(),
            UpgradePolicy {
                second_approver: true,
                ..UpgradePolicy::default()
            },
        );
    }
    let in_run = |app: &str, bundle: &str, version: &str, class| {
        let mut cmd = upgrade_to(app, "cora", bundle, Some(("p", version)));
        if let SystemCmd::UpgradeApp {
            class: c,
            owner_approves,
            run,
            ..
        } = &mut cmd
        {
            *c = class;
            *owner_approves = true;
            *run = Some("u".into());
        }
        cmd
    };
    // The instance is judged over the unit: green is refused, the unit's red applies.
    assert_eq!(
        m.run_unproven(in_run("inst", "b1", "1.0.1", Some(green()))),
        Err(UPGRADE_CLASS_MISMATCH),
        "a unit red through its member, logged green"
    );
    let _ok = m
        .run_unproven(in_run("inst", "b1", "1.0.1", Some(red(&telemetry))))
        .unwrap();
    // The member follows: its own policy does not stop the unit half way.
    let _ok = m
        .run_unproven(in_run("mem", "b2", "1.1.0", Some(red(&telemetry))))
        .unwrap();
    assert_eq!(m.state.apps["mem"].bundle, "b2");
    // Alone, the member's own policy holds.
    let mut alone = classified();
    instance_of(
        &mut alone,
        "mem",
        "b1",
        json!({ "package": "p", "version": "1.0.0" }),
    );
    if let Some(o) = alone.state.orgs.get_mut("cust") {
        let _prev = o.app_upgrade_policies.insert(
            "mem".into(),
            UpgradePolicy {
                second_approver: true,
                ..UpgradePolicy::default()
            },
        );
    }
    let mut cmd = upgrade_to("mem", "cora", "b2", Some(("p", "1.1.0")));
    if let SystemCmd::UpgradeApp {
        class,
        owner_approves,
        ..
    } = &mut cmd
    {
        *class = Some(red(&telemetry));
        *owner_approves = true;
    }
    assert_eq!(alone.run_unproven(cmd), Err(SECOND_APPROVAL_REQUIRED));
}

/// Units nest: a parent unit's first approval may cover a member that is itself the
/// instance of a unit. A live first approval of exactly that member's unit (the same
/// targets) still lets its run through, whichever approval sorts first.
#[test]
fn a_units_own_approval_is_found_past_a_parent_units() {
    use crate::types::{PendingApproval, RunTarget, UpgradePolicy, UpgradeRun};
    let telemetry = ["https://telemetry.example/"];
    let mut m = classified();
    // `inst` created `mid`, which created `leaf`; all on p@1.0.0.
    for id in ["mid", "leaf"] {
        instance_of(
            &mut m,
            id,
            "b1",
            json!({ "package": "p", "version": "1.0.0" }),
        );
    }
    for (parent, name, member) in [("inst", "mid", "mid"), ("mid", "leaf", "leaf")] {
        if let Some(a) = m.state.apps.get_mut(parent) {
            let _prev = a.links.insert(name.into(), member.into());
        }
    }
    let to = |app: &str| -> std::collections::BTreeMap<String, RunTarget> {
        std::iter::once((
            app.to_owned(),
            RunTarget {
                package: "p".into(),
                version: "1.1.0".into(),
            },
        ))
        .collect()
    };
    let approval = |targets| PendingApproval {
        package: "p".into(),
        version: "1.1.0".into(),
        targets,
        by: "cora".into(),
        at: 5,
        class: red(&telemetry),
    };
    if let Some(o) = m.state.orgs.get_mut("cust") {
        o.upgrade_policy = Some(UpgradePolicy {
            second_approver: true,
            ..UpgradePolicy::default()
        });
        // cora approved both units first: `inst`'s (which covers `mid`, and sorts
        // first) and `mid`'s own.
        let _prev = o
            .pending_upgrades
            .insert("inst".into(), approval(to("mid")));
        let _prev = o
            .pending_upgrades
            .insert("mid".into(), approval(to("leaf")));
        let _prev = o.upgrade_runs.insert(
            "u".into(),
            UpgradeRun {
                run_id: "u".into(),
                package: "p".into(),
                version: "1.1.0".into(),
                team: None,
                cascade: false,
                by: "eve".into(),
                started_at: 9,
                apps: vec!["mid".into(), "leaf".into()],
                done: Vec::new(),
                failed: std::collections::BTreeMap::new(),
                finished: false,
                targets: to("leaf"),
            },
        );
    }
    // eve applies `mid`'s unit, which cora approved.
    let mut cmd = upgrade_to("mid", "eve", "b2", Some(("p", "1.1.0")));
    if let SystemCmd::UpgradeApp {
        class,
        owner_approves,
        run,
        ..
    } = &mut cmd
    {
        *class = Some(red(&telemetry));
        *owner_approves = true;
        *run = Some("u".into());
    }
    let _ok = m.run_unproven(cmd).unwrap();
    let rec = m.state.apps["mid"].last_upgrade.clone().unwrap();
    assert_eq!(rec.approved_by.as_deref(), Some("cora"));
}

/// Only a unit's consent (`__upgrade_unit`) states member targets: an organization's
/// run keeps none, whatever its entry lists, so an app it moves is never taken for a
/// unit's member and judged on its own policy.
#[test]
fn only_a_units_run_keeps_member_targets() {
    use super::SECOND_APPROVAL_REQUIRED;
    let (mut m, mut alice) = org_with_package_apps();
    // `p1` created `p1b`, whose own policy asks for a second approver.
    let _prev = m.state.apps.insert(
        "p1b".into(),
        serde_json::from_value(json!({
            "appId": "p1b", "name": "p1b", "group": 7, "bundle": "b-1.0.0", "owner": "alice",
            "org": "org", "grants": [{ "kind": "org" }], "replicas": [1, 2, 3],
            "createdAt": 0, "release": { "package": "p", "version": "1.0.0" },
        }))
        .unwrap(),
    );
    if let Some(a) = m.state.apps.get_mut("p1") {
        let _prev = a.links.insert("store".into(), "p1b".into());
    }
    if let Some(o) = m.state.orgs.get_mut("org") {
        let _prev = o.app_upgrade_policies.insert(
            "p1b".into(),
            crate::types::UpgradePolicy {
                second_approver: true,
                ..crate::types::UpgradePolicy::default()
            },
        );
    }
    m.index = 10;
    let _ok = m.run(SystemCmd::RequireCurrentRules).unwrap();
    m.index = 11;
    // An organization's run whose entry adds targets its consent never stated.
    let c = consent(
        &mut alice,
        "org:org",
        "__upgrade_org",
        &json!({
            "package": "p", "version": "2.0.0", "team": null,
            "cascade": false, "migrate": null,
        }),
    );
    let run: crate::types::UpgradeRun = serde_json::from_value(json!({
        "runId": "r1", "package": "p", "version": "2.0.0", "by": "alice",
        "startedAt": 0, "apps": [], "done": [], "failed": {}, "finished": false,
        "targets": { "p1b": { "package": "p", "version": "2.0.0" } },
    }))
    .unwrap();
    let _ok = m
        .run(SystemCmd::StartUpgradeRun {
            org: "org".into(),
            run: Box::new(run),
            consent: c,
            by_release: false,
            owner_approves: false,
        })
        .unwrap();
    let upgrade = |app: &str| {
        let mut cmd = upgrade_to(app, "alice", "b-2.0.0", Some(("p", "2.0.0")));
        if let SystemCmd::UpgradeApp { run, .. } = &mut cmd {
            *run = Some("r1".into());
        }
        cmd
    };
    m.index = 12;
    let _ok = m.run(upgrade("p1")).unwrap();
    m.index = 13;
    assert_eq!(
        m.run(upgrade("p1b")).unwrap_err(),
        SECOND_APPROVAL_REQUIRED,
        "judged on its own policy"
    );
    assert!(
        m.state.orgs["org"].upgrade_runs["r1"].targets.is_empty(),
        "targets no consent stated"
    );
}

/// An update on its own that lost to another move of the same app (the app runs its
/// target already, a pass on another node or a person got there first) is not the
/// release's failure: its `UpgradeFailed` changes nothing at apply, so the record keeps
/// the move that won and the release stays eligible.
#[test]
fn an_automatic_failure_that_lost_to_another_move_records_nothing() {
    let mut m = auto_fixture();
    automatic(&mut m, 24);
    let hold_over = 1000 + 24 * 3600;
    let _ok = m
        .run(auto_upgrade(
            "1.0.2",
            "b3",
            Some(green()),
            hold_over,
            Some(taken_on(1, 4)),
        ))
        .unwrap();
    assert!(m.state.apps["inst"].last_upgrade.as_ref().unwrap().ok);
    let to = |version: &str| PackageRef {
        package: "p".into(),
        version: version.into(),
        service: None,
        compute: false,
    };
    // The losing pass's note, once the app runs what it was moving to.
    let _ok = m
        .run_unproven(SystemCmd::UpgradeFailed {
            app_id: "inst".into(),
            bundle: "b3".into(),
            by: super::AUTO_UPDATE.into(),
            at: hold_over + 1,
            error: json!({ "code": "UPGRADE_FAILED" }),
            run: None,
            release: Some(to("1.0.2")),
            from_release: None,
        })
        .unwrap();
    let last = m.state.apps["inst"].last_upgrade.clone().unwrap();
    assert!(
        last.ok && last.auto,
        "the winning move stays recorded: {last:?}"
    );
    // One that began from a release the app no longer runs is stale too, whatever it
    // was moving to; one that began from where the app is, the release's own, is noted.
    let failed = |version: &str, from: &str| SystemCmd::UpgradeFailed {
        app_id: "inst".into(),
        bundle: "b9".into(),
        by: super::AUTO_UPDATE.into(),
        at: hold_over + 2,
        error: json!("trap"),
        run: None,
        release: Some(to(version)),
        from_release: Some(to(from)),
    };
    assert_eq!(
        failed("1.1.0", "1.0.0").features(),
        [super::cluster_version::Feature::AutoUpdates]
    );
    let _ok = m.run_unproven(failed("1.1.0", "1.0.0")).unwrap();
    assert!(m.state.apps["inst"].last_upgrade.as_ref().unwrap().ok);
    let _ok = m.run_unproven(failed("1.1.0", "1.0.2")).unwrap();
    let last = m.state.apps["inst"].last_upgrade.clone().unwrap();
    assert!(!last.ok, "the release's own failure is noted: {last:?}");
}

/// An app whose record names no release (`release` absent: a record from before, or its
/// release inferred from its bundle) runs the release `running` derives. An automatic
/// update names that one as where it began, and apply judges it against the same: its own
/// failure is noted, one that another move overtook is not. An automatic failure logged
/// without `from_release` is judged on its target alone, as before the field.
#[test]
fn an_automatic_failure_is_judged_on_the_derived_release_of_an_app_without_one() {
    let mut m = auto_fixture();
    automatic(&mut m, 24);
    instance_of(&mut m, "inst", "b1", Value::Null);
    let to = |version: &str| PackageRef {
        package: "p".into(),
        version: version.into(),
        service: None,
        compute: false,
    };
    let inst = m.state.apps["inst"].clone();
    assert!(inst.release.is_none());
    // Where an update on its own begins: the release it runs, derived.
    assert_eq!(m.state.auto_update_from(&inst), Some(to("1.0.0")));
    let failed = |from: Option<&str>| SystemCmd::UpgradeFailed {
        app_id: "inst".into(),
        bundle: "b3".into(),
        by: super::AUTO_UPDATE.into(),
        at: 90_000,
        error: json!("trap"),
        run: None,
        release: Some(to("1.0.2")),
        from_release: from.map(to),
    };
    // Its own failure, from where it is: noted.
    let _ok = m.run_unproven(failed(Some("1.0.0"))).unwrap();
    let last = m.state.apps["inst"].last_upgrade.clone();
    assert!(
        last.as_ref().is_some_and(|u| !u.ok),
        "the release's own failure is noted: {last:?}"
    );
    // A failure from a release it never ran (another move won): nothing.
    m.state.apps.get_mut("inst").unwrap().last_upgrade = None;
    let _ok = m.run_unproven(failed(Some("1.0.1"))).unwrap();
    assert!(m.state.apps["inst"].last_upgrade.is_none(), "stale");
    // An entry without `from_release` is noted unless the app runs the target.
    let _ok = m.run_unproven(failed(None)).unwrap();
    assert!(m.state.apps["inst"]
        .last_upgrade
        .as_ref()
        .is_some_and(|u| !u.ok));
}

/// `inst` with a member `kv` of package `q` on 0.1.0, under the automatic policy:
/// `p` 1.0.1's lock pins `kv` at 0.1.1 (it would move the member), 1.0.2's at 0.1.0.
fn auto_unit_fixture() -> SystemMachine {
    let mut m = auto_fixture();
    automatic(&mut m, 24);
    let mut q = m.state.packages["p"].clone();
    q.name = "q".into();
    q.versions.clear();
    for version in ["0.1.0", "0.1.1"] {
        let _prev = q.versions.insert(
            version.into(),
            serde_json::from_value(json!({
                "version": version, "mpk": version, "mpkSize": 1, "source": "upload",
                "services": { "": "bq" }, "installedBy": "pam", "installedAt": 0
            }))
            .unwrap(),
        );
    }
    let _prev = m.state.packages.insert("q".into(), q);
    instance_of(
        &mut m,
        "kv",
        "bq",
        json!({ "package": "q", "version": "0.1.0" }),
    );
    let _prev = m
        .state
        .apps
        .get_mut("inst")
        .unwrap()
        .links
        .insert("kv".into(), "kv".into());
    let p = m.state.packages.get_mut("p").unwrap();
    for (version, kv) in [("1.0.1", "0.1.1"), ("1.0.2", "0.1.0")] {
        let _prev = p.versions.get_mut(version).unwrap().uses.insert(
            "kv".into(),
            super::ReleaseUse {
                package: "q".into(),
                range: "^0.1".into(),
                version: Some(kv.into()),
            },
        );
    }
    m
}

/// An update on its own moves the instance alone, so it is refused where its release
/// would move the unit's members: by an entry that says so (`members_stay`), or once the
/// logged cluster version brings the rule. An entry logged before either applies as it did.
#[test]
fn an_update_on_its_own_that_would_move_the_units_members_is_refused() {
    use super::cluster_version::Feature;
    use super::AUTO_UPDATE_REFUSED;
    let hold_over = 1000 + 24 * 3600;
    let moving = |members_stay: bool| {
        let mut cmd = auto_upgrade(
            "1.0.1",
            "b3",
            Some(green()),
            hold_over,
            Some(taken_on(1, 7)),
        );
        if let SystemCmd::UpgradeApp {
            members_stay: m, ..
        } = &mut cmd
        {
            *m = members_stay;
        }
        cmd
    };
    assert!(moving(true)
        .features()
        .contains(&Feature::AutoUpdateMembersStay));

    let mut m = auto_unit_fixture();
    assert_eq!(m.run(moving(true)), Err(AUTO_UPDATE_REFUSED));
    // Logged before the rule: applies as it did, and the member stays behind.
    let mut before = auto_unit_fixture();
    let _ok = before.run(moving(false)).unwrap();
    assert_eq!(before.state.apps["inst"].bundle, "b3");
    assert_eq!(
        before.state.apps["kv"].release.as_ref().unwrap().version,
        "0.1.0"
    );

    // The rule in force: the leader leaves 1.0.1 out, and an unmarked entry is refused too.
    let versions = |m: &SystemMachine| -> Vec<String> {
        m.state.auto_update_candidates(hold_over)[0]
            .1
            .iter()
            .map(|r| r.version.clone())
            .collect()
    };
    assert_eq!(versions(&m), ["1.0.2", "1.0.1"]);
    // rc.38 also brings who may use a release: the owner uses these, as public ones.
    for v in m.state.packages.get_mut("p").unwrap().versions.values_mut() {
        v.visibility = Some(super::Visibility::Public);
    }
    m.state.cluster_version = Some(Feature::PrivateReleases.since().into());
    assert_eq!(versions(&m), ["1.0.2"]);
    assert_eq!(m.run(moving(false)), Err(AUTO_UPDATE_REFUSED));
    // A release whose lock keeps the member where it is still applies on its own.
    let mut keeping = auto_upgrade(
        "1.0.2",
        "b3",
        Some(green()),
        hold_over,
        Some(taken_on(1, 7)),
    );
    if let SystemCmd::UpgradeApp { members_stay, .. } = &mut keeping {
        *members_stay = true;
    }
    let _ok = m.run(keeping).unwrap();
    assert_eq!(
        m.state.apps["inst"].release.as_ref().unwrap().version,
        "1.0.2"
    );
}

/// A member's release is chosen as `unit_members` chooses it for the instance's owner: a
/// private release installed for another organization never makes an update on its own
/// move the unit's members.
#[test]
fn another_orgs_private_member_release_does_not_refuse_an_update_on_its_own() {
    use super::cluster_version::Feature;
    let mut m = auto_unit_fixture();
    let hold_over = 1000 + 24 * 3600;
    m.state.cluster_version = Some(Feature::PrivateReleases.since().into());
    let q = m.state.packages.get_mut("q").unwrap();
    q.signer_id = "did:key:other".into();
    for v in q.versions.values_mut() {
        v.visibility = Some(super::Visibility::Public);
    }
    let private: super::PackageVersion = serde_json::from_value(json!({
        "version": "0.1.2", "mpk": "0.1.2", "mpkSize": 1, "source": "registry",
        "services": { "": "bq" }, "installedBy": "zed", "installedAt": 0,
        "visibility": "private", "orgs": [p("other")],
    }))
    .unwrap();
    let _prev = q.versions.insert("0.1.2".into(), private);
    // The member runs 0.1.1; 1.0.2's lock names no release, only the range.
    m.state.apps.get_mut("kv").unwrap().release = Some(super::PackageRef {
        package: "q".into(),
        version: "0.1.1".into(),
        service: None,
        compute: false,
    });
    let p = m.state.packages.get_mut("p").unwrap();
    for v in p.versions.values_mut() {
        v.visibility = Some(super::Visibility::Public);
    }
    p.versions
        .get_mut("1.0.2")
        .unwrap()
        .uses
        .get_mut("kv")
        .unwrap()
        .version = None;
    let to = super::PackageRef {
        package: "p".into(),
        version: "1.0.2".into(),
        service: None,
        compute: false,
    };
    assert!(
        !m.state.moves_members(&m.state.apps["inst"], &to),
        "0.1.2 is another organization's"
    );
    m.state
        .auto_update_allows(
            &m.state.apps["inst"],
            Some(&to),
            Some(&green()),
            hold_over,
            true,
        )
        .unwrap();
}

/// The account that uploaded a raw bundle still names it after an organization installs a
/// private release shipping the same WASM; no one else gains it.
#[test]
fn an_uploader_keeps_a_raw_bundle_a_private_release_later_ships() {
    let mut m = SystemMachine::default();
    let _ok = m.run_authorized(genesis(all(Authority::Owner))).unwrap();
    for who in ["alice", "bob"] {
        let _ok = m.run_authorized(account(who)).unwrap();
    }
    let _prev = m.state.bundles.insert(
        "b9".into(),
        serde_json::from_value(json!({ "hash": "b9", "size": 1, "uploadedBy": "alice" })).unwrap(),
    );
    assert!(m.state.sees_bundle("b9", "alice") && m.state.sees_bundle("b9", "bob"));
    let _ok = m
        .run_authorized(SystemCmd::InstallPackage {
            package: "p".into(),
            signer_id: "did:key:z".into(),
            release: serde_json::from_value(json!({
                "version": "1.0.0", "mpk": "ab", "mpkSize": 1, "source": "registry",
                "services": { "": "b9" }, "installedBy": "bob", "installedAt": 1,
                "visibility": "private", "orgs": [p("bob")],
            }))
            .unwrap(),
        })
        .unwrap();
    assert!(m.state.sees_bundle("b9", "bob"));
    assert!(m.state.sees_bundle("b9", "alice"), "she uploaded it");
    assert!(!m.state.sees_bundle("b9", OPERATOR));
}

/// A deployment owned by an organization serves a private release only if one who acts
/// for it is in an organization the release is private to, a public or catalog one always,
/// and judges a release recorded before `visibility` member by member.
#[test]
fn an_org_deployment_serves_a_private_release_of_its_own_org_alone() {
    use super::{Deployer, Deployment};
    let mut m = SystemMachine::default();
    let _ok = m.run_authorized(genesis(all(Authority::Owner))).unwrap();
    for who in ["alice", "bob"] {
        let _ok = m.run_authorized(account(who)).unwrap();
    }
    // alice owns A and bob owns B; alice is also a member of B, where she manages nothing.
    let _prev = m
        .state
        .orgs
        .get_mut(&p("bob"))
        .unwrap()
        .members
        .insert("alice".into(), OrgRole::Member);
    let deployment = |org: &str| Deployment {
        id: org.into(),
        name: org.into(),
        owner: Deployer::Org(p(org)),
        apps: BTreeMap::new(),
        created_by: org.into(),
        created_at: 0,
    };
    let release = |extra: Value| -> super::PackageVersion {
        let mut v = json!({
            "version": "1.0.0", "mpk": "ab", "mpkSize": 1, "source": "registry",
            "services": { "": "b1" }, "installedBy": "alice", "installedAt": 1,
        });
        v.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        serde_json::from_value(v).unwrap()
    };
    let (a, b) = (deployment("alice"), deployment("bob"));
    let private = |orgs: &[&str]| {
        release(json!({
            "visibility": "private", "orgs": orgs.iter().map(|o| p(o)).collect::<Vec<_>>(),
        }))
    };
    assert!(m.state.offer_serves(&a, &private(&["alice"])));
    assert!(!m.state.offer_serves(&b, &private(&["alice"])), "not B's");
    assert!(m.state.offer_serves(&b, &private(&["alice", "bob"])));
    assert!(!m.state.offer_serves(&a, &private(&["zed"])), "no one of A");
    assert!(
        m.state.offer_serves(&a, &private(&["bob"])),
        "alice is in B"
    );
    assert!(m
        .state
        .offer_serves(&b, &release(json!({ "visibility": "public" }))));
    let catalog = release(json!({ "visibility": "private", "installedBy": OPERATOR }));
    assert!(m.state.offer_serves(&b, &catalog));
    // Before `visibility`: whoever shares an organization with an installer.
    assert!(m.state.offer_serves(&b, &release(json!({}))));
    assert!(!m
        .state
        .offer_serves(&b, &release(json!({ "installedBy": "carol" }))));
}
