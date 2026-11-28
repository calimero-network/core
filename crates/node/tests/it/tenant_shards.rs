//! The control plane sharded by tenant, end to end: each organization's apps
//! live on its own tenant group (allocated from its own block of group ids),
//! root mirrors them for its machinery, and a move between organizations hands
//! an app from one tenant to the other with its data
//! ([design note](https://github.com/calimero-network/core/blob/master/docs/design/system-group-sharding.md),
//! "The `Apps` flip: tenants authoritative, root mirrors").
//!
//! With the cutover off (`shard::ACTIVE`, `Node::sharding_active`) the same
//! flow runs on the one system group, and only the shard checks are skipped.

use calimero_client::types::OrgRole;
use calimero_client::Client;
use serde_json::{json, Value};

use crate::component_cluster::{eventually, mpk_declaring};
use crate::harness::{kv_wasm, signed_up, typed, wire, Cluster};

const PACKAGE: &str = "com.calimero.tenant-kv";

async fn org(owner: &Client, name: &str) -> String {
    owner
        .orgs()
        .create(name, None, None)
        .await
        .map(wire)
        .unwrap()["orgId"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// An instance of the kv release in `org`, installed for it first.
async fn kv_in(owner: &Client, org: &str) -> String {
    let publisher = ed25519_dalek::SigningKey::from_bytes(&[9; 32]);
    let _ = owner
        .packages()
        .upload(
            mpk_declaring(PACKAGE, "0.1.0", &kv_wasm(), &publisher, &[]),
            Some(org),
        )
        .await
        .unwrap();
    owner
        .apps()
        .create(typed(
            json!({ "name": "kv", "package": PACKAGE, "org": org }),
        ))
        .await
        .map(wire)
        .unwrap()["appId"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn put(cl: &Client, app: &str, key: &str) {
    let _set = cl
        .apps()
        .call(
            app,
            "set",
            Some(json!({ "key": key, "value": format!("{key}-value") })),
            None,
            None,
        )
        .await
        .unwrap();
}

async fn holds(cl: &Client, app: &str, key: &str) {
    let want = json!(format!("{key}-value"));
    let got = eventually(cl, app, "get", json!({ "key": key }), |v| *v == want).await;
    assert_eq!(got, want, "{app} keeps {key}");
}

/// Which organization each node's tenant of `org` says `app` is in, and
/// root's mirror on node 0.
fn shards(c: &Cluster, org: &str, app: &str) -> (Vec<Option<String>>, Option<String>) {
    let tenants = c
        .nodes
        .iter()
        .flatten()
        .map(|r| r.node.tenant_app_org(org, app))
        .collect();
    let mirror = c.nodes[0]
        .as_ref()
        .unwrap()
        .node
        .sys(|s| s.apps.get(app).map(|a| a.org.clone()));
    (tenants, mirror)
}

/// Two organizations' apps each live on their own tenant group, in its own
/// block of group ids, mirrored on root; moved from one to the other, an app
/// leaves the first tenant for the second with its data.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn each_organizations_apps_live_on_its_tenant_and_move_with_their_data() {
    let c = Cluster::start(3).await;
    let (alice, mut alice_pk) = signed_up(&c, 0, "alice").await;
    let (bob, mut bob_pk) = signed_up(&c, 1, "bob").await;
    let acme = org(&alice, "Acme").await;
    let globex = org(&bob, "Globex").await;
    let a = kv_in(&alice, &acme).await;
    let b = kv_in(&bob, &globex).await;
    put(&alice, &a, "alpha").await;
    put(&bob, &b, "beta").await;
    holds(&alice, &a, "alpha").await;
    holds(&bob, &b, "beta").await;

    let sharded = calimero_node::Node::sharding_active();
    let group = |app: &str| c.nodes[0].as_ref().unwrap().node.sys(|s| s.apps[app].group);
    if sharded {
        // Each tenant allocates its apps' groups from its own block.
        let (ga, gb) = (group(&a), group(&b));
        assert!(ga >= 1 << 40 && gb >= 1 << 40, "{ga} {gb}");
        assert!(ga.abs_diff(gb) >= 1 << 32, "{ga} {gb}: one block each");
        // Each app is its own organization's, on its tenant and in root's mirror.
        let in_acme = Some(acme.clone());
        assert_eq!(shards(&c, &acme, &a), (vec![in_acme.clone(); 3], in_acme));
        assert_eq!(shards(&c, &globex, &a).0, vec![None; 3]);
        let in_globex = Some(globex.clone());
        assert_eq!(
            shards(&c, &globex, &b),
            (vec![in_globex.clone(); 3], in_globex)
        );
    }

    // Alice is made an admin of Globex and moves her app there.
    let alice_id = wire(alice.auth().me().await.unwrap())["accountId"]
        .as_str()
        .unwrap()
        .to_owned();
    let _ = bob
        .orgs()
        .set_member(&globex, &alice_id, OrgRole::Admin, Some(&mut bob_pk))
        .await
        .unwrap();
    let moved: Value = alice
        .apps()
        .app_move(&a, &globex, Some(false), Some(&mut alice_pk))
        .await
        .map(wire)
        .unwrap();
    assert_eq!(moved["org"], json!(globex), "{moved}");
    holds(&alice, &a, "alpha").await;
    if sharded {
        let in_globex = Some(globex.clone());
        crate::harness::poll::eventually(
            crate::harness::poll::EVENTUALLY,
            "the app is Globex's on every shard",
            || {
                let got = shards(&c, &globex, &a);
                let gone = shards(&c, &acme, &a).0;
                let want = (vec![in_globex.clone(); 3], in_globex.clone());
                async move {
                    if got == want && gone == vec![None; 3] {
                        Ok(())
                    } else {
                        Err((got, gone))
                    }
                }
            },
        )
        .await;
    }
}
