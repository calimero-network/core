//! What one organization and one account keep in the system state is bounded: teams per
//! organization (`maxTeamsPerOrg`) and metadata bytes per organization and per owning account
//! (`maxMetadataBytesPerOrg`, `maxMetadataBytesPerAccount`), refused `429 QUOTA_EXCEEDED`; and
//! `GET /orgs/{org}` answers its teams and members in pages.
//! [Quotas](https://calimero-network.github.io/core/protocol/limits/#quotas)

use calimero_client::types::TeamJoin;
use calimero_client::Client;
use serde_json::{json, Value};

use crate::harness;

use harness::{signed_up, typed, wire, Cluster};

fn s(v: &Value) -> String {
    v.as_str()
        .unwrap_or_else(|| panic!("not a string: {v}"))
        .to_owned()
}

async fn new_team(
    c: &Client,
    org: &str,
    name: &str,
) -> Result<String, Box<calimero_client::Error>> {
    let v = c
        .orgs()
        .team_create(org, name, TeamJoin::Invite, None)
        .await
        .map(wire)?;
    Ok(s(&v["teamId"]))
}

/// JSON metadata of `bytes` bytes.
fn metadata(bytes: usize) -> Value {
    json!("x".repeat(bytes - 2))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn teams_and_metadata_are_bounded_per_organization_and_account() {
    let c = Cluster::start(3).await;
    let ops = c.client(0);
    ops.use_api_key("ops-secret").await;
    let _ok = ops
        .ops()
        .quotas_set(
            typed(json!({
                "maxTeamsPerOrg": 2,
                "maxMetadataBytesPerOrg": 30_000,
                "maxMetadataBytesPerAccount": 40_000,
            })),
            Some(&harness::operator_sign),
        )
        .await
        .unwrap();
    let (alice, _a) = signed_up(&c, 0, "alice").await;
    let org = s(&alice
        .orgs()
        .create("Acme", None, None)
        .await
        .map(wire)
        .unwrap()["orgId"]);
    let org2 = s(&alice
        .orgs()
        .create("Acme Two", None, None)
        .await
        .map(wire)
        .unwrap()["orgId"]);

    let t1 = new_team(&alice, &org, "one").await.unwrap();
    let _t2 = new_team(&alice, &org, "two").await.unwrap();
    let err = new_team(&alice, &org, "three").await.unwrap_err();
    assert_eq!(err.code(), Some("QUOTA_EXCEEDED"), "{err}");

    // 15 000 + 15 000 fit the organization's 30 000; a third does not.
    let _ok = alice.orgs().metadata(&org, metadata(15_000)).await.unwrap();
    let _ok = alice
        .orgs()
        .team_update(&org, &t1, None, None, None, Some(metadata(15_000)))
        .await
        .unwrap();
    let me = s(&alice.auth().me().await.map(wire).unwrap()["accountId"]);
    let err = alice
        .orgs()
        .member_metadata(&org, &me, metadata(100))
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("QUOTA_EXCEEDED"), "{err}");
    // Alice's other organization fits its own bound, not hers: 30 000 + 15 000 > 40 000.
    let err = alice
        .orgs()
        .metadata(&org2, metadata(15_000))
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("QUOTA_EXCEEDED"), "{err}");
    let _ok = alice
        .orgs()
        .metadata(&org2, metadata(10_000))
        .await
        .unwrap();
    // Shrinking always passes.
    let _ok = alice.orgs().metadata(&org, metadata(10)).await.unwrap();

    let o = alice
        .orgs()
        .get(&org, None, None, None)
        .await
        .map(wire)
        .unwrap();
    assert_eq!(o["teams"].as_object().unwrap().len(), 2, "{o}");
    assert_eq!(o["metadata"], metadata(10));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn an_organization_answers_its_teams_and_members_in_pages() {
    let c = Cluster::start(3).await;
    let (alice, mut a) = signed_up(&c, 0, "alice").await;
    let (bob, _b) = signed_up(&c, 1, "bob").await;
    let org = s(&alice
        .orgs()
        .create("Acme", None, None)
        .await
        .map(wire)
        .unwrap()["orgId"]);
    let token = alice
        .orgs()
        .invite_create(&org, typed(json!({ "role": "member" })), Some(&mut a))
        .await
        .map(wire)
        .unwrap();
    let _r = bob.orgs().invite_redeem(&s(&token["token"])).await.unwrap();
    let mut teams = Vec::new();
    for name in ["one", "two", "three"] {
        teams.push(new_team(&alice, &org, name).await.unwrap());
    }
    teams.sort();

    let all = alice
        .orgs()
        .get(&org, None, None, None)
        .await
        .map(wire)
        .unwrap();
    assert_eq!(all["teams"].as_object().unwrap().len(), 3, "{all}");
    assert_eq!(all["members"].as_object().unwrap().len(), 2, "{all}");
    assert_eq!(all["nextTeamsAfter"], Value::Null, "{all}");
    assert_eq!(all["nextMembersAfter"], Value::Null, "{all}");

    // One team a page, then one member a page.
    let (mut paged, mut after) = (Vec::new(), None::<String>);
    loop {
        let page = alice
            .orgs()
            .get(&org, Some(1), after.as_deref(), None)
            .await
            .map(wire)
            .unwrap();
        let ids = page["teams"].as_object().unwrap();
        assert_eq!(ids.len(), 1, "{page}");
        paged.extend(ids.keys().cloned());
        after = page["nextTeamsAfter"].as_str().map(str::to_owned);
        let Some(next) = &after else { break };
        assert_eq!(Some(next), paged.last(), "{page}");
    }
    assert_eq!(paged, teams);
    let (mut members, mut after) = (Vec::new(), None::<String>);
    loop {
        let page = alice
            .orgs()
            .get(&org, Some(1), None, after.as_deref())
            .await
            .map(wire)
            .unwrap();
        let ids = page["members"].as_object().unwrap();
        assert_eq!(ids.len(), 1, "{page}");
        members.extend(ids.keys().cloned());
        after = page["nextMembersAfter"].as_str().map(str::to_owned);
        if after.is_none() {
            break;
        }
    }
    let want: Vec<String> = all["members"]
        .as_object()
        .unwrap()
        .keys()
        .cloned()
        .collect();
    assert_eq!(members, want);

    let err = alice
        .orgs()
        .get(&org, Some(1001), None, None)
        .await
        .unwrap_err();
    assert_eq!(err.code(), Some("BAD_REQUEST"), "{err}");
}
