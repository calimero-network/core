//! The typed answers write the bytes the `json!` they replaced wrote. Each test keeps the
//! replaced expression, fed representative values, reads its text into the answer's type and
//! writes it back: the same text, key order and `null` against absent included, or the type
//! would answer something else.

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Value};

/// `before` (the replaced `json!`) read into `T` and written again is the same text.
fn writes_as_before<T: DeserializeOwned + Serialize>(before: &Value) {
    let typed: T = serde_json::from_value(before.clone())
        .unwrap_or_else(|e| panic!("{} reads {before}: {e}", std::any::type_name::<T>()));
    assert_eq!(
        serde_json::to_string(&typed).expect("serializes"),
        serde_json::to_string(before).expect("serializes"),
        "{}",
        std::any::type_name::<T>()
    );
}

fn status() -> Value {
    json!({
        "group": 2, "leader": 1, "is_leader": true, "term": 3, "last_log": 9, "applied": 9,
        "voters": [1, 2, 3], "learners": [],
    })
}

fn receipt() -> Value {
    json!({
        "app": "a1", "index": 4, "resultHash": "ab", "node": 1, "key": "cd", "signature": "ef",
    })
}

#[test]
fn calls_answer_as_before() {
    let events = vec![json!({ "kind": "posted", "data": { "n": 1 } })];
    writes_as_before::<super::calls::IntentResult>(
        &json!({ "index": 4, "result": [1], "events": events, "receipt": receipt() }),
    );
    writes_as_before::<super::calls::ViewResult>(
        &json!({ "index": 4, "result": null, "receipt": receipt() }),
    );
    writes_as_before::<super::calls::PresenceList>(&json!({ "present": [
        { "account": "a", "device": "d", "state": null },
        { "account": "b", "device": "e", "state": "away" },
    ] }));
    writes_as_before::<super::calls::RunLogs>(&json!({ "logs": [] }));
    writes_as_before::<crate::node::RunList>(&json!({ "runs": [], "next": null }));
    writes_as_before::<crate::component::XcallsReport>(&json!({
        "pending": 2,
        "deadLetters": [{ "seq": 1, "app": "b", "method": "m", "reason": "gone", "atMs": 5 }],
    }));
}

#[test]
fn upgrade_answers_answer_as_before() {
    for (status, error) in [("ok", Value::Null), ("appError", json!("no"))] {
        writes_as_before::<super::upgrades::UpgradeApplied>(
            &json!({ "index": 7, "status": status, "error": error, "compute": false }),
        );
    }
    let approval = json!({
        "app": "a1",
        "package": "p",
        "version": "2.0.0",
        "targets": { "a2": { "package": "q", "version": "1.1.0" } },
        "by": "alice",
        "at": 10,
        "expiresAt": 604_810,
        "class": "red",
        "reasons": [{ "code": "egressAdded", "destinations": ["api.example.com"] }],
    });
    writes_as_before::<super::update_policies::ApprovalLogged>(
        &json!({ "app": "a1", "approval": approval }),
    );
    let shown = json!({ "id": "d1", "name": "Acme", "owner": { "kind": "org", "name": "acme" } });
    for (running, deployment) in [
        (Value::Null, Value::Null),
        (json!({ "package": "p", "version": "1.0.0" }), shown),
    ] {
        writes_as_before::<super::upgrades::UpdateView>(&json!({
            "app": "a1",
            "running": running,
            "available": { "package": "p", "version": "2.0.0", "source": "release" },
            "unit": false,
            "mayApprove": true,
            "approvers": "owner",
            "policy": { "autoApply": "never", "holdHours": 0,
                "secondApprover": false, "source": "default" },
            "pendingApprovals": [approval],
            "deployment": deployment,
        }));
    }
    let unit = json!({
        "destinations": ["api.example.com"],
        "members": { "kv": { "package": "kv", "version": "1.0.0", "signer": "did:key:z",
            "destinations": [], "app": "a2" } },
    });
    let mut on = unit.clone();
    on["app"] = json!("a1");
    on["egress"] = json!("on");
    writes_as_before::<super::upgrades::EgressView>(&on);
    let mut off = on.clone();
    off["egress"] = json!("off");
    off["offBy"] = json!("organization");
    off["summary"] = json!(crate::egress::NO_EGRESS);
    writes_as_before::<super::upgrades::EgressView>(&off);
    writes_as_before::<crate::egress::UnitEgress>(&unit);
}

#[test]
fn app_answers_answer_as_before() {
    writes_as_before::<super::apps::AppAttestations>(&json!({
        "appId": "a1",
        "cluster": "c",
        "replicas": [
            { "node": 1, "attestation": { "quote": "q" } },
            { "node": 2, "error": "timed out" },
        ],
    }));
    writes_as_before::<super::apps::AppCreated>(&json!({
        "appId": "a1", "contextId": "ab", "group": 3, "replicas": [1, 2],
        "compute": true, "links": { "kv": "a2" },
    }));
    writes_as_before::<super::apps::AppList>(&json!({ "apps": [{
        "appId": "a1", "name": "n", "owner": "o", "org": "g", "bundle": "b",
        "package": null, "version": null, "deployment": null,
    }] }));
    writes_as_before::<super::apps::AppDeleted>(
        &json!({ "appId": "a1", "deleted": true, "members": ["a2"] }),
    );
    writes_as_before::<super::apps::AppInviteMade>(&json!({ "token": "t" }));
    writes_as_before::<super::apps::AppInviteList>(&json!({ "invites": [{
        "inviteKey": "k", "usesLeft": 1, "expiresAt": 9, "expired": false,
        "createdBy": "o", "createdAt": 1, "scope": null,
    }] }));
    for at in [Value::Null, json!(12)] {
        writes_as_before::<super::apps::AppStorage>(&json!({
            "keys": 1, "bytes": 2, "blobs": 0, "blobBytes": 0, "lastWriteAt": at,
        }));
    }
    writes_as_before::<super::apps::AppAbi>(&json!({ "bundle": "b", "abi": null }));
    writes_as_before::<super::apps::AppOpenApi>(&json!({ "openapi": "3.1.0", "info": {} }));
    let day = json!({ "date": "2026-11-25", "runs": "1", "fuel": "2", "wallMs": "3",
        "writes": "4", "bytesWritten": "5", "events": "6" });
    writes_as_before::<super::apps::AppUsage>(&json!({ "today": day, "days": [day] }));
    writes_as_before::<super::apps::DelegationMade>(
        &json!({ "source": "a1", "target": "a2", "methods": ["m"] }),
    );
    writes_as_before::<super::apps::Delegations>(
        &json!({ "source": "a1", "delegations": { "a2": ["m"] } }),
    );
    // The row as the handler built it: the declared fields, the optional ones appended, then
    // the instance's own value's.
    let mut row = json!({
        "host": "api.example.com", "paths": ["/v1"], "binding": "b", "description": "",
        "required": true, "source": "cluster",
    });
    writes_as_before::<super::apps::AppSecrets>(&json!({ "secrets": { "KEY": row } }));
    row["header"] = json!("Authorization");
    row["source"] = json!("app");
    row["fingerprint"] = json!("0123456789abcdef");
    row["setBy"] = json!("account:a");
    row["setAt"] = json!(5);
    writes_as_before::<super::apps::AppSecrets>(&json!({ "secrets": { "KEY": row } }));
    writes_as_before::<super::apps::SecretSet>(
        &json!({ "name": "KEY", "fingerprint": "f", "index": 3 }),
    );
    writes_as_before::<super::apps::SecretCleared>(&json!({ "name": "KEY", "index": 3 }));
    let link = json!({ "name": "kv", "package": "kv", "range": "^1", "mode": "create",
        "app": "a2", "version": "1.0.0", "inRange": true });
    let gone = json!({ "name": "old", "package": null, "range": null, "mode": null,
        "app": "a3", "version": null, "inRange": null });
    writes_as_before::<super::apps::AppLinkList>(&json!({ "app": "a1", "links": [link, gone] }));
    writes_as_before::<crate::system::AppMoved>(&json!({
        "appId": "a1", "from": "o1", "org": "o2", "grants": [{ "kind": "org" }],
        "created": true, "added": ["b"], "revokedInvites": 0,
    }));
}

#[test]
fn undo_answers_answer_as_before() {
    let part = json!({
        "app": "a1",
        "from": { "package": "p", "version": "2.0.0" },
        "to": { "package": "p", "version": "1.0.0" },
        "toBundle": "b",
        "upgradedAt": 10,
        "upgradedBy": "alice",
        "auto": false,
        "undoUntil": 604_810,
        "restorePoint": { "node": 1, "index": 4, "sha256": "ab" },
        "undoneAt": null,
        "lostWrites": { "entriesSince": 0, "lastWriteAt": null, "any": false },
        "unavailable": null,
    });
    writes_as_before::<super::undo::UndoView>(&json!({
        "app": "a1",
        "available": false,
        "reason": { "code": "NO_RESTORE_POINT", "message": "none" },
        "apps": [],
    }));
    writes_as_before::<super::undo::UndoView>(&json!({
        "app": "a1",
        "available": true,
        "reason": null,
        "undoUntil": 604_810,
        "apps": [part],
    }));
    writes_as_before::<super::undo::UndoDone>(&json!({
        "app": "a1", "groups": { "a1": 7 }, "restored": true, "apps": [part],
    }));
}

#[test]
fn pipeline_and_file_answers_answer_as_before() {
    writes_as_before::<super::pipelines::RunEstimate>(&json!({
        "steps": { "a": { "runs": "1", "fuel": "2" } }, "fuel": "2",
    }));
    writes_as_before::<super::pipelines::RunEstimate>(&json!({
        "steps": { "a": { "runs": "1", "fuel": "2", "cost": "3" } }, "fuel": "2",
        "cost": "3", "priceVersion": 1,
    }));
    writes_as_before::<super::pipelines::PipelineStarted>(&json!({ "run": "r" }));
    writes_as_before::<super::pipelines::PipelineStarted>(
        &json!({ "run": "r", "worstCase": "9", "retryOf": "q" }),
    );
    writes_as_before::<super::pipelines::PipelineRunList>(
        &json!({ "runs": [{ "run": "r", "state": "running", "steps": {} }] }),
    );
    writes_as_before::<super::pipelines::PipelineCancelled>(&json!({ "run": "r" }));
    writes_as_before::<crate::node::SignedPipelineReceipt>(
        &json!({ "receipt": { "v": 1 }, "node": 1, "signature": "ab" }),
    );
    writes_as_before::<super::exports::FileExport>(&json!({
        "app": "a1", "id": "f", "size": 3, "sha256": "ab", "segments": ["cd"],
        "ephemeral": "ef", "wrapped": "01",
    }));
    writes_as_before::<crate::packages::pipelines::ReleasePipelines>(&json!({
        "p": { "hash": "h", "worstCaseFuel": "9", "modules": {}, "pipeline": { "steps": {} } },
    }));
    writes_as_before::<super::packages::CapabilityCard>(&json!({
        "package": "p", "version": "1.0.0", "service": null, "bundle": "b", "mpk": "m",
        "manifestHash": "h", "signer": "s", "name": null, "author": null, "outbound": [],
        "secretBindings": [], "secrets": {}, "uses": {}, "abi": null,
        "destinations": null, "members": null,
    }));
}

#[test]
fn ops_answers_answer_as_before() {
    writes_as_before::<super::ops::ClusterIdentity>(&json!({ "cluster": "c", "identity": "ab" }));
    writes_as_before::<super::ops::JobNodeVerdict>(
        &json!({ "state": "admitted", "node": 4, "release": "1.0.0" }),
    );
    writes_as_before::<super::ops::JobNodeVerdict>(
        &json!({ "state": "refused", "node": 4, "reason": "no" }),
    );
    writes_as_before::<super::ops::OpsStatus>(&json!({
        "node": 1,
        "system": status(),
        "governance": { "releases": "owner", "appUpgrades": "owner", "outbound": "owner" },
        "groups": [{ "app": "a1", "status": status() }],
        "nodes": [{ "id": 1, "addr": "http://n1", "zone": "", "role": "replica" }],
        "apps": 1,
        "accounts": 2,
        "rollback": null,
    }));
    writes_as_before::<super::ops::Placement>(&json!({
        "fresh": true,
        "apps": [{ "app": "a1", "group": 2, "replicas": [1] }],
        "nodes": [{ "id": 1, "zone": "z", "role": "replica", "release": null, "apps": 1 }],
        "clusterVersion": null,
    }));
    let governance = json!({ "releases": "owner", "appUpgrades": "owner", "outbound": "owner" });
    writes_as_before::<super::ops::OpsApps>(&json!({
        "fresh": true, "governance": governance, "apps": [{ "app": "a1" }], "next": null,
    }));
    writes_as_before::<super::ops::OpsOrgs>(&json!({
        "fresh": true,
        "governance": governance,
        "orgs": [{ "org": "o", "approvedReleases": null }, { "org": "p", "approvedReleases": ["1"] }],
        "next": "p",
    }));
    writes_as_before::<super::ops::NodeRemoved>(&json!({ "status": status() }));
    writes_as_before::<super::ops::ReplicasSet>(&json!({ "status": status(), "unchanged": true }));
    writes_as_before::<super::ops::PriceTableView>(&json!({}));
    writes_as_before::<super::ops::PriceTableView>(&json!(calimero_pipeline::PriceTable {
        version: 1,
        fuel_per_vcpu_second: 2,
        vcpu_second: 3,
        gib_second: 4,
        connector_gib: 5,
        storage_gib_month: 6,
        egress_gib: 7,
    }));
    writes_as_before::<super::ops::OpsPools>(&json!({
        "pools": { "gpu": { "runsPerNode": 1, "maxMemoryMib": 2 } }, "capacity": { "gpu": 2 },
    }));
    writes_as_before::<super::ops::BackupWritten>(&json!({ "written": 3 }));
    writes_as_before::<super::origins::ServedOrigins>(&json!({
        "fresh": true,
        "origins": [{ "origin": "https://a.b", "host": "a.b", "label": "p", "addedAt": "2026-11-25T00:00:00Z" }],
    }));
    writes_as_before::<super::account_proof::ReceiptKey>(
        &json!({ "node": 2, "key": "ab", "release": null, "attestedAt": 5 }),
    );
    writes_as_before::<crate::system::GovernanceSet>(&json!({ "governance": governance }));
    writes_as_before::<crate::system::ClusterOwnerNamed>(&json!({ "owner": { "account": "a" } }));
}

#[test]
fn report_and_restore_answers_answer_as_before() {
    for scope in [
        json!({ "kind": "org", "org": "o", "personal": true }),
        json!({ "kind": "org", "org": "o" }),
        json!({ "kind": "account" }),
    ] {
        writes_as_before::<super::AppsReport>(&json!({
            "scope": scope,
            "governance": { "releases": "owner", "appUpgrades": "owner", "outbound": "owner" },
            "approvedReleases": null,
            "clusterReleases": [{ "release": "1.0.0", "nodes": 3, "approved": true }],
            "apps": [{ "app": "a1", "lastWriteAt": null }],
            "next": null,
        }));
    }
    let restored = json!({
        "app": "a1",
        "group": 5,
        "rows": 10,
        "backup": { "name": "b", "index": 4, "format": 1, "merod": null, "release": null },
        "status": status(),
    });
    writes_as_before::<crate::node::restore::AppRestored>(&restored);
    writes_as_before::<super::restore::HolderRecovered>(&json!({ "apps": [restored] }));
    writes_as_before::<super::restore::RecoveryFiles>(&json!({
        "holder": { "kind": "org", "id": "o" },
        "files": [{ "name": "f", "header": "{}", "signature": "ab" }],
    }));
    writes_as_before::<crate::node::restore::SystemRecovered>(
        &json!({ "apps": 1, "accounts": 2, "index": 3 }),
    );
}

#[test]
fn outbound_log_answers_as_before() {
    let sent = json!({
        "atMs": 1_700_000_000_000_u64, "method": "GET", "host": "echo.outbound.test",
        "path": "/v1/enrich", "pathTruncated": false, "status": 200, "secrets": ["ECHO_KEY"],
        "requestBytes": 0, "responseBytes": 12,
    });
    let refused = json!({
        "atMs": 1_700_000_000_001_u64, "method": "POST", "host": "", "path": "",
        "pathTruncated": false, "failure": "notApproved", "secrets": [],
        "requestBytes": 3, "responseBytes": 0,
    });
    writes_as_before::<calimero_outbound::AuditEntry>(&sent);
    writes_as_before::<calimero_outbound::AuditEntry>(&refused);
    for partial in [false, true] {
        writes_as_before::<crate::node::OutboundLog>(
            &json!({ "entries": [refused.clone(), sent.clone()], "partial": partial }),
        );
        writes_as_before::<crate::node::OutboundLog>(&json!({ "entries": [], "partial": partial }));
    }
}
