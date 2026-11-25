//! Public links (`/apps/{app}/public-links`) and the anonymous visitors they let in: a link of
//! one instance whose token, with a visitor's own Ed25519 key, calls that instance's
//! `#[app::public]` methods as `Caller::Visitor`, whatever its access. Nothing about a visitor
//! is logged; a link is kept under its token's public key (`proof::public_link_key`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[allow(
    clippy::wildcard_imports,
    reason = "the handlers read `api.rs`'s items, as every area does"
)]
use super::*;
use crate::system::cluster_version::Feature;
use crate::types::{LinkGrant, PublicLink};

/// What starts a visitor's bearer credential: `vis1.<link token>.<visitor key>.<at>.<sig>`.
pub(super) const VISITOR_PREFIX: &str = "vis1.";
/// The longest visitor credential read (a token, a key, a time and a signature).
const MAX_VISITOR_CREDENTIAL_BYTES: usize = 512;
/// Link and visitor pairs whose bucket one node keeps at once.
const MAX_TRACKED_VISITORS: usize = 16_384;
/// `429`: a visitor called through a link more often than its `ratePerMinute`.
const TOO_MANY_VISITS: &str = "TOO_MANY_VISITS";
/// `403`: a visitor's credential on a route that is not one of its instance's public methods.
pub(super) const VISITOR_ONLY: &str = "VISITOR_ONLY";

/// A token bucket per (link, visitor key): the link's `ratePerMinute` tokens, refilled evenly
/// over a minute. In memory, per node.
#[derive(Default)]
pub struct VisitorLimits(Mutex<HashMap<(String, String), (f64, Instant)>>);

impl VisitorLimits {
    /// Takes a token from the visitor's bucket of `link`; false when it is empty, or the node
    /// tracks too many visitors to start another.
    fn take(&self, link: &str, key: &str, per_minute: u32) -> bool {
        let rate = f64::from(per_minute.max(1));
        let now = Instant::now();
        let mut buckets = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let refill = |(tokens, at): &(f64, Instant)| {
            (tokens + now.duration_since(*at).as_secs_f64() * rate / 60.0).min(rate)
        };
        let id = (link.to_owned(), key.to_owned());
        if buckets.len() >= MAX_TRACKED_VISITORS && !buckets.contains_key(&id) {
            buckets.retain(|_, b| b.1.elapsed().as_secs() < 60);
            if buckets.len() >= MAX_TRACKED_VISITORS {
                return false;
            }
        }
        let bucket = buckets.entry(id).or_insert((rate, now));
        let tokens = refill(bucket);
        if tokens < 1.0 {
            *bucket = (tokens, now);
            return false;
        }
        *bucket = (tokens - 1.0, now);
        true
    }
}

/// The visitor a `vis1.` credential names, checked: the visitor signed the proof over the link,
/// its key and a time near now, and then, on fresh system state (the request's read-index
/// round, which a credential whose signature does not hold never costs), the link exists. Who the link admits, and to
/// which method, is [`visitor_gate`]'s.
pub(super) async fn visitor(node: &Node, rest: &str) -> Result<Principal, ApiError> {
    if rest.len() > MAX_VISITOR_CREDENTIAL_BYTES {
        return Err(ApiError::unauthenticated("malformed visitor credential"));
    }
    let mut parts = rest.split('.');
    let (Some(token), Some(key), Some(at), Some(signature), None) = (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) else {
        return Err(ApiError::unauthenticated("malformed visitor credential"));
    };
    let at: u64 = at
        .parse()
        .map_err(|_| ApiError::unauthenticated("malformed visitor credential"))?;
    let link = crate::proof::public_link_key(token);
    crate::proof::verify_visitor(&link, key, at, signature, now_secs())
        .map_err(ApiError::unauthenticated)?;
    // Only a credential whose signature holds costs a read-index round.
    let (sys_index, _caught_up) = node.sys_fresh_index().await;
    if !node.sys(|s| s.public_link(&link).is_some()) {
        return Err(ApiError::unauthenticated(crate::system::PUBLIC_LINK_GONE));
    }
    Ok(Principal {
        caller: Caller::Visitor {
            link,
            key: key.to_owned(),
        },
        session: None,
        api_key: None,
        issuer_release: None,
        consent_page: false,
        sys_index,
    })
}

/// What a visitor may do on `app`: call or view `method`, a method of the instance its link
/// is of, while the link admits visitors (fresh state: not revoked or expired, the cluster
/// version brings public links, the organization allows them), within the link's rate. The
/// ABI's `public` flag is judged where the call runs (`node::abi_check`). Any other route or
/// app answers as to a stranger.
pub(super) async fn visitor_gate(
    node: &Node,
    p: &Principal,
    app: &crate::types::AppRecord,
    method: &str,
) -> Result<(), ApiError> {
    let Caller::Visitor { link, key } = &p.caller else {
        return Ok(());
    };
    if method == "*" || crate::node::reserved_method(method) {
        return Err(ApiError::new(
            403,
            VISITOR_ONLY,
            "a visitor through a public link calls only the instance's public methods",
        ));
    }
    node.require_features(&[Feature::PublicLinks]).await?;
    let admitted = node.sys(|s| {
        s.public_link_admits(link, now_secs())
            .map(|(a, l)| (a.app_id.clone(), l.clone()))
    });
    let Some((of, l)) = admitted else {
        return Err(ApiError::unauthenticated(
            "this public link no longer admits visitors",
        ));
    };
    if of != app.app_id {
        return Err(ApiError::not_found("no such app"));
    }
    if !node.visitor_limits.take(link, key, l.rate_per_minute) {
        return Err(ApiError::new(
            429,
            TOO_MANY_VISITS,
            "too many calls through this link: wait a minute",
        ));
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(test, derive(utoipa::ToSchema))]
pub(super) struct PublicLinkBody {
    /// A name its managers know it by.
    #[serde(default)]
    #[cfg_attr(test, schema(nullable = false))]
    label: Option<String>,
    /// When it stops admitting visitors (Unix seconds); none: until revoked.
    #[serde(default)]
    #[cfg_attr(test, schema(nullable = false))]
    expires_at: Option<u64>,
    /// Calls one visitor may make through it a minute (default 60, at most 600).
    #[serde(default)]
    #[cfg_attr(test, schema(nullable = false))]
    rate_per_minute: Option<u32>,
    #[serde(default)]
    #[cfg_attr(test, schema(nullable = false))]
    confirm: Option<ConfirmBody>,
}

/// A link as its list shows it: its key and terms, never a token.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
#[cfg_attr(test, derive(utoipa::ToSchema))]
pub(super) struct LinkView {
    link: String,
    #[cfg_attr(test, schema(required = true))]
    label: Option<String>,
    #[cfg_attr(test, schema(required = true))]
    expires_at: Option<u64>,
    expired: bool,
    rate_per_minute: u32,
    created_by: String,
    created_at: u64,
}

impl LinkView {
    fn of(l: &LinkGrant, now: u64) -> Self {
        Self {
            link: l.link.clone(),
            label: l.label.clone(),
            expires_at: l.expires_at,
            expired: l.expires_at.is_some_and(|at| at <= now),
            rate_per_minute: l.rate_per_minute,
            created_by: l.created_by.clone(),
            created_at: l.created_at,
        }
    }
}

/// `GET /apps/{app}/public-links`.
#[derive(Serialize)]
#[cfg_attr(test, derive(utoipa::ToSchema))]
pub(super) struct PublicLinks {
    links: Vec<LinkView>,
}

/// `DELETE /apps/{app}/public-links/{link}`: `{}`.
#[derive(Serialize)]
#[cfg_attr(test, derive(utoipa::ToSchema))]
pub(super) struct Done {}

/// `POST /apps/{app}/public-links`: the link's key and, this once, its token.
#[derive(Serialize)]
#[cfg_attr(test, derive(utoipa::ToSchema))]
pub(super) struct PublicLinkMade {
    link: String,
    token: String,
}

/// Make a public link of an instance: whoever may change its access, with their passkey over
/// the link's key and terms (`412` first). The token is answered once; the log keeps only its
/// public key.
#[cfg_attr(test, utoipa::path(
    post,
    path = "/apps/{app}/public-links",
    params(("app" = String, Path)),
    request_body = PublicLinkBody,
    responses((status = 200, description = "OK", body = PublicLinkMade)),
))]
pub(super) async fn public_link_create(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
    Json(body): Json<PublicLinkBody>,
) -> ApiResult<Json<PublicLinkMade>> {
    let app_id = node.canonical_app_id(&app_id);
    let p = holder(&node, &headers).await?;
    let actor = p.account()?;
    node.sys_fresh_from(p.sys_index).await;
    let app = node.app(&app_id).await?;
    node.sys(|s| {
        if !s.may_reach(&app, Some(&actor)) {
            return Err(ApiError::not_found("no such app"));
        }
        if !s.manages_grants(&app, &actor) {
            return Err(ApiError::forbidden(
                "only whoever may change the instance's access makes its public links",
            ));
        }
        if !s.orgs.get(&app.org).is_some_and(|o| o.allow_public_links) {
            return Err(ApiError::new(
                409,
                "PUBLIC_LINKS_OFF",
                crate::system::PUBLIC_LINKS_OFF,
            ));
        }
        Ok(())
    })?;
    let rate = body
        .rate_per_minute
        .unwrap_or(crate::system::DEFAULT_PUBLIC_LINK_RATE);
    if rate == 0 || rate > crate::system::MAX_PUBLIC_LINK_RATE {
        return Err(ApiError::bad_request(
            "BAD_REQUEST",
            "ratePerMinute is 1 to 600",
        ));
    }
    if body.label.as_ref().is_some_and(|l| {
        l.chars().count() > crate::system::MAX_PUBLIC_LINK_LABEL_CHARS
            || l.chars().any(char::is_control)
    }) {
        return Err(ApiError::bad_request(
            "BAD_REQUEST",
            "a label is one line of at most 64 characters",
        ));
    }
    let now = now_secs();
    if body.expires_at.is_some_and(|at| at <= now) {
        return Err(ApiError::bad_request(
            "BAD_REQUEST",
            "expiresAt is in the past",
        ));
    }
    node.require_features(&[Feature::PublicLinks]).await?;
    let token = format!("pl_{}", b64url(&random::<32>()));
    let link = PublicLink {
        app: app_id.clone(),
        label: body.label,
        expires_at: body.expires_at,
        rate_per_minute: rate,
        created_by: actor.clone(),
        created_at: now,
    };
    let args = crate::system::public_link_args(&crate::proof::public_link_key(&token), &link);
    let c = consent_minted(
        &node,
        &p,
        &app_id,
        "__create_public_link",
        &args,
        Some(json!(token)),
        body.confirm,
    )
    .await?;
    // The retry records the link the approval names, minted by the first request.
    let token = c
        .carry
        .as_ref()
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| ApiError::bad_request("WRONG_CHALLENGE", "not a public link approval"))?;
    let link_key = crate::proof::public_link_key(&token);
    if crate::system::public_link_args(&link_key, &link) != c.args {
        return Err(ApiError::bad_request(
            "WRONG_CHALLENGE",
            "the approval was for a different call",
        ));
    }
    let _ok = node
        .sys_write(&SystemCmd::CreatePublicLink {
            link_key: link_key.clone(),
            link,
            actor,
            consent: c.consent,
        })
        .await?;
    Ok(Json(PublicLinkMade {
        link: link_key,
        token,
    }))
}

/// The instance's public links, for whoever may change its access: keys and terms, never a
/// token. `404` to anyone else.
#[cfg_attr(test, utoipa::path(
    get,
    path = "/apps/{app}/public-links",
    params(("app" = String, Path)),
    responses((status = 200, description = "OK", body = PublicLinks)),
))]
pub(super) async fn public_link_list(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
) -> ApiResult<Json<PublicLinks>> {
    let app_id = node.canonical_app_id(&app_id);
    let p = signed_in(&node, &headers).await?;
    let actor = p.account()?;
    node.sys_fresh_from(p.sys_index).await;
    let app = node.app(&app_id).await?;
    let now = now_secs();
    node.sys(|s| {
        if !s.may_reach(&app, Some(&actor)) || !s.manages_grants(&app, &actor) {
            return Err(ApiError::not_found("no such app"));
        }
        let links = app
            .access
            .links()
            .into_iter()
            .map(|l| LinkView::of(l, now))
            .collect();
        Ok(Json(PublicLinks { links }))
    })
}

/// Revoke a public link: its creator or whoever may change the instance's access, with their
/// passkey (`412` first). It admits nobody from the next request.
#[cfg_attr(test, utoipa::path(
    delete,
    path = "/apps/{app}/public-links/{link}",
    params(("app" = String, Path), ("link" = String, Path)),
    request_body(content = Option<ConfirmOnlyBody>),
    responses((status = 200, description = "OK", body = Done)),
))]
pub(super) async fn public_link_revoke(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((app_id, link)): Path<(String, String)>,
    body: Option<Json<ConfirmOnlyBody>>,
) -> ApiResult<Json<Done>> {
    let app_id = node.canonical_app_id(&app_id);
    let p = holder(&node, &headers).await?;
    let actor = p.account()?;
    node.sys_fresh_from(p.sys_index).await;
    let app = node.app(&app_id).await?;
    let visible = node.sys(|s| {
        app.access
            .link(&link)
            .is_some_and(|l| l.created_by == actor || s.manages_grants(&app, &actor))
    });
    if !visible {
        return Err(ApiError::not_found(crate::system::PUBLIC_LINK_GONE));
    }
    let args = json!({ "linkKey": link });
    let consent = consent(
        &node,
        &p,
        &app_id,
        "__revoke_public_link",
        &args,
        body.and_then(|Json(b)| b.confirm),
    )
    .await?;
    let _ok = node
        .sys_write(&SystemCmd::RevokePublicLink {
            app_id,
            link_key: link,
            actor,
            consent,
        })
        .await?;
    Ok(Json(Done {}))
}

/// This area's routes, merged into [`super::router`].
pub(super) fn routes() -> Router<Arc<Node>> {
    Router::new()
        .route(
            "/apps/{app}/public-links",
            post(public_link_create).get(public_link_list),
        )
        .route(
            "/apps/{app}/public-links/{link}",
            delete(public_link_revoke),
        )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{LinkGrant, LinkView, PublicLinkMade};

    /// The typed answers write the bytes the `json!` they replaced wrote, key order included.
    #[test]
    fn the_typed_link_answers_write_what_they_wrote_before() {
        let with: LinkGrant = serde_json::from_value(json!({
            "link": "k1", "label": "Team page", "expiresAt": 100,
            "ratePerMinute": 60, "createdBy": "alice", "createdAt": 7
        }))
        .expect("a link");
        let without: LinkGrant = serde_json::from_value(json!({
            "link": "k2", "ratePerMinute": 5, "createdBy": "bob", "createdAt": 8
        }))
        .expect("a link");
        for (l, now) in [(&with, 99), (&with, 100), (&without, 1)] {
            // The answer as `link_json` built it before.
            let before = json!({
                "link": l.link,
                "label": l.label,
                "expiresAt": l.expires_at,
                "expired": l.expires_at.is_some_and(|at| at <= now),
                "ratePerMinute": l.rate_per_minute,
                "createdBy": l.created_by,
                "createdAt": l.created_at,
            });
            assert_eq!(
                serde_json::to_string(&LinkView::of(l, now)).expect("serializes"),
                serde_json::to_string(&before).expect("serializes"),
            );
        }
        let made = PublicLinkMade {
            link: "k1".to_owned(),
            token: "pl_t".to_owned(),
        };
        assert_eq!(
            serde_json::to_string(&made).expect("serializes"),
            r#"{"link":"k1","token":"pl_t"}"#
        );
        assert_eq!(
            serde_json::to_string(&super::Done {}).expect("serializes"),
            "{}"
        );
    }
}
