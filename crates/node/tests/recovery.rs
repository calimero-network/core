//! Account recovery (`docs/specs/account-recovery.md`) through the API, on a
//! three-node cluster: a person who lost every passkey gets back in with the
//! recovery code they made while signed in. The code starts a recovery with
//! a new passkey; for `RECOVERY_DELAY_SECS` (72 hours, as an organization's
//! recovery) a passkey the account still has cancels it; after, the new
//! passkey finishes it, which ends every session the account had and signs
//! in.

use calimero_client::{Client, Passkey, Session};
use calimero_identity::testing::SoftAuthenticator;
use calimero_node::types::RECOVERY_DELAY_SECS;
use serde_json::{json, Value};

#[path = "harness/cluster.rs"]
#[allow(
    dead_code,
    reason = "shared with tests/cluster.rs, which uses the parts this test does not"
)]
mod harness;

use harness::{signed_up, Cluster, ORIGIN};

/// Moves every node's clock by `secs`, as `tests/org_recovery.rs` does.
fn advance(c: &Cluster, secs: u64) {
    let ms = i64::try_from(secs * 1000).unwrap();
    for r in c.nodes.iter().flatten() {
        r.node.advance_clock(ms);
    }
}

/// A new recovery code for the signed-in `client`, approved with `passkey`.
async fn make_code(client: &Client, passkey: &mut SoftAuthenticator) -> String {
    client
        .post_confirmed("auth/recovery", json!({}), passkey)
        .await
        .unwrap()["code"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// Begin and start a recovery with `code` and a new passkey `device`,
/// signed out on `client`: the pending recovery, or the refusal's code.
async fn start(
    client: &Client,
    code: &str,
    device: &mut SoftAuthenticator,
) -> Result<Value, String> {
    let refused = |e: calimero_client::Error| e.code().unwrap_or("?").to_owned();
    let begin = client
        .post("auth/recovery/begin", json!({ "code": code }))
        .await
        .map_err(refused)?;
    let registration = device.create(&begin["options"]);
    client
        .post(
            "auth/recovery/start",
            json!({ "challengeId": begin["challengeId"], "code": code, "registration": registration }),
        )
        .await
        .map_err(refused)
}

/// Make a code (412 step-up); start with it on another node; finishing
/// early is refused; the old passkey cancels; the spent code is refused; a
/// fresh code starts again, and once the delay passed the new passkey
/// finishes: the same account, the old sessions out, and the new passkey
/// signs in anywhere.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_recovery_code_brings_back_an_account_after_the_delay_unless_cancelled() {
    let c = Cluster::start(3).await;
    let (alice, mut old) = signed_up(&c, 0, "alice").await;
    let me = alice.me().await.unwrap();
    let account = me["accountId"].clone();
    assert_eq!(me["recoveryCode"], json!(false));
    assert_eq!(me["recoveryPending"], Value::Null);

    // Making a code is a credential change: 412 first, then the passkey.
    let e = alice.post("auth/recovery", json!({})).await.unwrap_err();
    assert_eq!(e.code(), Some("CONFIRM_REQUIRED"), "{e}");
    let first = make_code(&alice, &mut old).await;
    assert_eq!(alice.me().await.unwrap()["recoveryCode"], json!(true));

    // Someone with the code starts a recovery, signed out, on another node.
    let rescuer = c.client(1);
    let mut stolen = SoftAuthenticator::new("localhost", ORIGIN);
    // A passkey made on an app's (or anyone else's) origin is not taken.
    let mut elsewhere = SoftAuthenticator::new("localhost", "https://evil.example");
    let e = start(&rescuer, &first, &mut elsewhere).await.unwrap_err();
    assert_eq!(e, "BAD_REGISTRATION");
    let started = start(&rescuer, &first, &mut stolen).await.unwrap();
    assert_eq!(started["accountId"], account, "{started}");
    assert_eq!(
        started["readyAt"].as_u64().unwrap() - started["startedAt"].as_u64().unwrap(),
        RECOVERY_DELAY_SECS
    );
    assert!(
        started.get("sessionToken").is_none(),
        "no session: {started}"
    );
    // The account sees it; no passkey was added.
    let me = alice.me().await.unwrap();
    assert_eq!(
        me["recoveryPending"]["credentialId"],
        json!(stolen.credential_id()),
        "{me}"
    );
    assert_eq!(me["recoveryCode"], json!(false), "spent: {me}");
    assert_eq!(me["passkeys"].as_array().unwrap().len(), 1, "{me}");

    // Finishing before the delay is refused, named or not.
    let e = rescuer
        .post_confirmed(
            "auth/recovery/finish",
            json!({ "accountId": account }),
            &mut stolen,
        )
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("RECOVERY_NOT_READY"), "{e}");
    let e = rescuer
        .post_confirmed("auth/recovery/finish", json!({}), &mut stolen)
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("RECOVERY_NOT_READY"), "{e}");

    // The account's own passkey cancels it (412 first).
    let e = alice
        .post("auth/recovery/cancel", json!({}))
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("CONFIRM_REQUIRED"), "{e}");
    let _ok = alice
        .post_confirmed("auth/recovery/cancel", json!({}), &mut old)
        .await
        .unwrap();
    assert_eq!(alice.me().await.unwrap()["recoveryPending"], Value::Null);
    // Cancelled for good: even past the delay, nothing finishes.
    advance(&c, RECOVERY_DELAY_SECS);
    let e = rescuer
        .post_confirmed(
            "auth/recovery/finish",
            json!({ "accountId": account }),
            &mut stolen,
        )
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("NOT_FOUND"), "{e}");

    // The spent code is refused, on every node.
    for i in 0..3 {
        let e = c
            .client(i)
            .post("auth/recovery/begin", json!({ "code": first }))
            .await
            .unwrap_err();
        assert_eq!(e.code(), Some("UNAUTHENTICATED"), "node {i}: {e}");
    }

    // A fresh code; then every passkey is lost.
    let code = make_code(&alice, &mut old).await;
    drop(old);
    // The code as written down (lower case, spaces) starts the recovery.
    let typed = code.to_lowercase().replace('-', " ");
    let mut laptop = SoftAuthenticator::new("localhost", ORIGIN);
    let started = start(&rescuer, &typed, &mut laptop).await.unwrap();
    assert_eq!(started["accountId"], account, "{started}");
    let e = rescuer
        .post_confirmed(
            "auth/recovery/finish",
            json!({ "accountId": account }),
            &mut laptop,
        )
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("RECOVERY_NOT_READY"), "{e}");

    // Past the delay, the new passkey finishes it, on yet another node.
    advance(&c, RECOVERY_DELAY_SECS);
    let finisher = c.client(2);
    let session: Session = serde_json::from_value(
        finisher
            .post_confirmed(
                "auth/recovery/finish",
                json!({ "accountId": account }),
                &mut laptop,
            )
            .await
            .unwrap(),
    )
    .unwrap();
    finisher.use_session(session).await;
    let me = finisher.me().await.unwrap();
    assert_eq!(me["accountId"], account, "{me}");
    assert_eq!(me["passkeys"].as_array().unwrap().len(), 2, "{me}");
    assert_eq!(me["passkeyId"], json!(laptop.credential_id()), "{me}");
    assert_eq!(me["recoveryPending"], Value::Null, "{me}");
    assert_eq!(me["recoveryCode"], json!(false), "spent: {me}");

    // Whoever holds the lost device's session is out, refresh token too.
    let e = alice.me().await.unwrap_err();
    assert_eq!(e.code(), Some("UNAUTHENTICATED"), "{e}");
    assert!(alice.refresh().await.is_err());

    // The finish is spent: the same passkey cannot finish again.
    let e = c
        .client(0)
        .post_confirmed(
            "auth/recovery/finish",
            json!({ "accountId": account }),
            &mut laptop,
        )
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("NOT_FOUND"), "{e}");

    // The new passkey signs in anywhere, and makes the next code.
    let third = c.client(0);
    let _s = third.sign_in(&mut laptop).await.unwrap();
    assert_eq!(third.me().await.unwrap()["accountId"], account);
    let _code = make_code(&third, &mut laptop).await;
    assert_eq!(third.me().await.unwrap()["recoveryCode"], json!(true));
    // Removing it is a credential change too.
    let e = third.delete("auth/recovery").await.unwrap_err();
    assert_eq!(e.code(), Some("CONFIRM_REQUIRED"), "{e}");
    let _ok = third
        .delete_confirmed("auth/recovery", &mut laptop)
        .await
        .unwrap();
    assert_eq!(third.me().await.unwrap()["recoveryCode"], json!(false));
}
