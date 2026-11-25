//! The system log's answers write the bytes the `json!` they replaced wrote:
//! each test keeps the replaced expression and compares the serialized text, key order
//! included.

use serde::Serialize;
use serde_json::{json, Value};

use super::*;

fn same<T: Serialize>(typed: &T, before: &Value) {
    assert_eq!(
        serde_json::to_string(typed).expect("serializes"),
        serde_json::to_string(before).expect("serializes")
    );
}

#[test]
fn organization_answers_write_their_old_bytes() {
    let (org_id, team_id) = ("o1".to_owned(), "t1".to_owned());
    same(
        &OrgCreated {
            org_id: org_id.clone(),
            team_id: None,
        },
        &json!({ "orgId": org_id }),
    );
    let scoped = json!({ "teams": [{ "teamId": team_id }] });
    same(
        &OrgCreated {
            org_id: org_id.clone(),
            team_id: scoped["teams"][0]["teamId"].as_str().map(str::to_owned),
        },
        &json!({ "orgId": org_id, "teamId": scoped["teams"][0]["teamId"] }),
    );
    let apps = vec!["a1".to_owned(), "a2".to_owned()];
    same(&OrgDeleted { apps: apps.clone() }, &json!({ "apps": apps }));
    same(
        &OrgRenamed {
            org_id: org_id.clone(),
            name: "Org".to_owned(),
        },
        &json!({ "orgId": org_id, "name": "Org" }),
    );
    same(
        &TeamCreated {
            team_id: team_id.clone(),
        },
        &json!({ "teamId": team_id }),
    );
    let gone = vec![team_id.clone(), "t2".to_owned()];
    same(
        &TeamDeleted {
            teams: gone.clone(),
            apps: Vec::new(),
            kept_apps: Some(apps.clone()),
        },
        &json!({ "teams": gone, "apps": [], "keptApps": apps }),
    );
    same(
        &TeamDeleted {
            teams: gone.clone(),
            apps: apps.clone(),
            kept_apps: None,
        },
        &json!({ "teams": gone, "apps": apps }),
    );
    same(
        &OrgSettingsSet {
            org: org_id.clone(),
            allow_guests: false,
            allow_public_links: true,
        },
        &json!({ "org": org_id, "allowGuests": false, "allowPublicLinks": true }),
    );
    same(
        &PublisherBound { generation: 3 },
        &json!({ "generation": 2_u64 + 1 }),
    );
    same(
        &RecoveryStarted {
            started_at: 10,
            ready_at: 20,
        },
        &json!({ "startedAt": 10_u64, "readyAt": 20_u64 }),
    );
    let previous = vec!["bob".to_owned()];
    same(
        &RecoveryFinished {
            org: org_id.clone(),
            previous_owners: previous.clone(),
        },
        &json!({ "org": org_id, "previousOwners": previous }),
    );
    for join in [Some(TeamJoin::Everyone), None] {
        let instances = vec!["a1".to_owned()];
        let team = json!({
            "package": "p1",
            "teamId": team_id,
            "created": true,
            "apps": instances,
            "added": 2_usize,
            "join": join,
            "dropped": 0_usize,
        });
        let mode = crate::types::OrgMode::Standalone;
        same(
            &AppTeamsScoped {
                mode,
                teams: vec![ScopedTeam {
                    package: "p1".to_owned(),
                    team_id: team_id.clone(),
                    created: true,
                    apps: instances.clone(),
                    added: 2,
                    join,
                    dropped: 0,
                }],
            },
            &json!({ "mode": mode, "teams": [team] }),
        );
    }
}

#[test]
fn the_egress_answer_writes_its_old_bytes() {
    for app in [Some("a1".to_owned()), None] {
        let mut before = json!({ "org": "o1", "egress": false, "orgEgressOff": true });
        if let Some(id) = &app {
            before["app"] = json!(id);
        }
        same(
            &EgressSet {
                org: "o1".to_owned(),
                egress: false,
                org_egress_off: true,
                app,
            },
            &before,
        );
    }
}

#[test]
fn deployment_answers_write_their_old_bytes() {
    let deployer = Deployer::Org("o1".to_owned());
    let grant = DeployerGrant {
        until: Some(9),
        granted_at: 5,
        granted_by: "operator".to_owned(),
    };
    let key = deployer.key();
    same(
        &DeployerGranted {
            key: key.clone(),
            deployer: deployer.clone(),
            grant: grant.clone(),
        },
        &json!({ "key": key, "deployer": deployer, "grant": grant }),
    );
    same(
        &DeployerRevoked { key: key.clone() },
        &json!({ "key": key }),
    );
    let (deployment, app, name) = ("d1".to_owned(), "kv".to_owned(), "TOKEN".to_owned());
    let offer = Offer {
        package: "p1".to_owned(),
        version: "1.0.0".to_owned(),
        added_by: "alice".to_owned(),
        added_at: 5,
    };
    same(
        &AppOffered {
            deployment: deployment.clone(),
            app: app.clone(),
            offer: offer.clone(),
        },
        &json!({ "deployment": deployment, "app": app, "offer": offer }),
    );
    same(
        &AppWithdrawn {
            deployment: deployment.clone(),
            app: app.clone(),
        },
        &json!({ "deployment": deployment, "app": app }),
    );
    same(
        &DeploymentDeleted {
            deployment: deployment.clone(),
        },
        &json!({ "deployment": deployment }),
    );
    same(
        &DeploymentSecretSet {
            deployment: deployment.clone(),
            name: name.clone(),
            fingerprint: "fp".to_owned(),
            set_by: "account:alice".to_owned(),
            set_at: 7,
        },
        &json!({
            "deployment": deployment,
            "name": name,
            "fingerprint": "fp",
            "setBy": "account:alice",
            "setAt": 7_u64,
        }),
    );
    same(
        &DeploymentSecretCleared {
            deployment: deployment.clone(),
            name: name.clone(),
        },
        &json!({ "deployment": deployment, "name": name }),
    );
}
