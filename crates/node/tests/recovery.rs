//! Account recovery: a person who lost every passkey gets back in with the
//! recovery code they made while signed in. The code is made with a passkey
//! step-up and shown once; the log keeps only its public key. Recovering
//! adds a new passkey, spends the code and ends every session the account
//! had, on whichever node the person reaches.

use calimero_client::{Passkey, Session};
use calimero_identity::testing::SoftAuthenticator;
use serde_json::json;

#[path = "harness/cluster.rs"]
#[allow(
    dead_code,
    reason = "shared with tests/cluster.rs, which uses the parts this test does not"
)]
mod harness;

use harness::{signed_up, Cluster, ORIGIN};

/// A code made on one node recovers the account on another, once; a wrong
/// code ends the ceremony; a new code replaces the old; the old sessions end.
#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
async fn a_recovery_code_brings_back_an_account_that_lost_every_passkey() {
    let c = Cluster::start(3).await;
    let (alice, mut lost) = signed_up(&c, 0, "alice").await;
    let account = alice.me().await.unwrap()["accountId"].clone();
    assert_eq!(alice.me().await.unwrap()["recoveryCode"], json!(false));

    // Making a code is a credential change: 412 first, then the passkey.
    let e = alice.post("auth/recovery", json!({})).await.unwrap_err();
    assert_eq!(e.code(), Some("CONFIRM_REQUIRED"), "{e}");
    let first = alice
        .post_confirmed("auth/recovery", json!({}), &mut lost)
        .await
        .unwrap()["code"]
        .as_str()
        .unwrap()
        .to_owned();
    // A new one replaces it.
    let code = alice
        .post_confirmed("auth/recovery", json!({}), &mut lost)
        .await
        .unwrap()["code"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(first, code);
    assert_eq!(alice.me().await.unwrap()["recoveryCode"], json!(true));

    // Every passkey is lost. Signed out, on another node.
    drop(lost);
    let rescuer = c.client(1);
    let e = rescuer
        .post("auth/recovery/begin", json!({ "code": first }))
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("UNAUTHENTICATED"), "the replaced code: {e}");

    // A wrong code at finish ends the ceremony: the challenge is spent.
    let mut laptop = SoftAuthenticator::new("localhost", ORIGIN);
    let begin = rescuer
        .post("auth/recovery/begin", json!({ "code": code }))
        .await
        .unwrap();
    let registration = laptop.create(&begin["options"]);
    let e = rescuer
        .post(
            "auth/recovery/finish",
            json!({ "challengeId": begin["challengeId"], "code": first, "registration": registration }),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("UNAUTHENTICATED"), "a wrong code: {e}");
    let e = rescuer
        .post(
            "auth/recovery/finish",
            json!({ "challengeId": begin["challengeId"], "code": code, "registration": registration }),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("UNKNOWN_CHALLENGE"), "{e}");

    // A passkey made on an app's (or anyone else's) origin is not taken.
    let mut elsewhere = SoftAuthenticator::new("localhost", "https://evil.example");
    let begin = rescuer
        .post("auth/recovery/begin", json!({ "code": code }))
        .await
        .unwrap();
    let e = rescuer
        .post(
            "auth/recovery/finish",
            json!({ "challengeId": begin["challengeId"], "code": code, "registration": elsewhere.create(&begin["options"]) }),
        )
        .await
        .unwrap_err();
    assert_eq!(e.code(), Some("BAD_REGISTRATION"), "{e}");

    // The code as written down (lower case, spaces) recovers the account.
    let typed = code.to_lowercase().replace('-', " ");
    let begin = rescuer
        .post("auth/recovery/begin", json!({ "code": typed }))
        .await
        .unwrap();
    let session: Session = serde_json::from_value(
        rescuer
            .post(
                "auth/recovery/finish",
                json!({ "challengeId": begin["challengeId"], "code": typed, "registration": laptop.create(&begin["options"]) }),
            )
            .await
            .unwrap(),
    )
    .unwrap();
    rescuer.use_session(session).await;
    let me = rescuer.me().await.unwrap();
    assert_eq!(me["accountId"], account, "{me}");
    assert_eq!(me["passkeys"].as_array().unwrap().len(), 2, "{me}");
    assert_eq!(me["passkeyId"], json!(laptop.credential_id()), "{me}");
    assert_eq!(me["recoveryCode"], json!(false), "spent: {me}");

    // Whoever holds the lost device's session is out.
    let e = alice.me().await.unwrap_err();
    assert_eq!(e.code(), Some("UNAUTHENTICATED"), "{e}");

    // The code is single use, on every node.
    for i in 0..3 {
        let e = c
            .client(i)
            .post("auth/recovery/begin", json!({ "code": code }))
            .await
            .unwrap_err();
        assert_eq!(e.code(), Some("UNAUTHENTICATED"), "node {i}: {e}");
    }

    // The new passkey signs in anywhere, and can make the next code.
    let third = c.client(2);
    let _s = third.sign_in(&mut laptop).await.unwrap();
    assert_eq!(third.me().await.unwrap()["accountId"], account);
    let _code = third
        .post_confirmed("auth/recovery", json!({}), &mut laptop)
        .await
        .unwrap();
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
