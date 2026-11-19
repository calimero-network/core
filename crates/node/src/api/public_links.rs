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
use crate::types::PublicLink;

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

/// The visitor a `vis1.` credential names, checked: the link it names exists, and the visitor
/// signed the proof over the link, its key and a time near now. Who the link admits, and to
/// which method, is [`visitor_gate`]'s.
pub(super) fn visitor(
    node: &Node,
    rest: &str,
    sys_index: Option<u64>,
) -> Result<Principal, ApiError> {
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
    if !node.sys(|s| s.public_links.contains_key(&link)) {
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
    let admitted = node.sys(|s| s.public_link_admits(link, now_secs()).cloned());
    let Some(l) = admitted else {
        return Err(ApiError::unauthenticated(
            "this public link no longer admits visitors",
        ));
    };
    if l.app != app.app_id {
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
pub(super) struct PublicLinkBody {
    /// A name its managers know it by.
    #[serde(default)]
    label: Option<String>,
    /// When it stops admitting visitors (Unix seconds); none: until revoked.
    #[serde(default)]
    expires_at: Option<u64>,
    /// Calls one visitor may make through it a minute (default 60, at most 600).
    #[serde(default)]
    rate_per_minute: Option<u32>,
    #[serde(default)]
    confirm: Option<ConfirmBody>,
}

/// A link as its list shows it: its key and terms, never a token.
fn link_json(key: &str, l: &PublicLink, now: u64) -> Value {
    json!({
        "link": key,
        "label": l.label,
        "expiresAt": l.expires_at,
        "expired": l.expires_at.is_some_and(|at| at <= now),
        "ratePerMinute": l.rate_per_minute,
        "createdBy": l.created_by,
        "createdAt": l.created_at,
    })
}

/// Make a public link of an instance: whoever may change its access, with their passkey over
/// the link's key and terms (`412` first). The token is answered once; the log keeps only its
/// public key.
async fn public_link_create(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
    Json(body): Json<PublicLinkBody>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    let p = holder(&node, &headers).await?;
    let actor = p.account()?;
    node.sys_fresh_from(p.sys_index).await;
    let app = node.app(&app_id).await?;
    node.sys(|s| {
        if !s.may_reach(&app, Some(&actor)) {
            return Err(ApiError::not_found("no such app"));
        }
        if !s.manages_guests(&app, &actor) {
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
    Ok(Json(json!({ "link": link_key, "token": token })))
}

/// The instance's public links, for whoever may change its access: keys and terms, never a
/// token. `404` to anyone else.
async fn public_link_list(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    let p = signed_in(&node, &headers).await?;
    let actor = p.account()?;
    node.sys_fresh_from(p.sys_index).await;
    let app = node.app(&app_id).await?;
    let now = now_secs();
    node.sys(|s| {
        if !s.may_reach(&app, Some(&actor)) || !s.manages_guests(&app, &actor) {
            return Err(ApiError::not_found("no such app"));
        }
        let links: Vec<Value> = s
            .public_links
            .iter()
            .filter(|(_, l)| l.app == app_id)
            .map(|(k, l)| link_json(k, l, now))
            .collect();
        Ok(Json(json!({ "links": links })))
    })
}

/// Revoke a public link: its creator or whoever may change the instance's access, with their
/// passkey (`412` first). It admits nobody from the next request.
async fn public_link_revoke(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((app_id, link)): Path<(String, String)>,
    body: Option<Json<ConfirmOnlyBody>>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    let p = holder(&node, &headers).await?;
    let actor = p.account()?;
    node.sys_fresh_from(p.sys_index).await;
    let app = node.app(&app_id).await?;
    let visible = node.sys(|s| {
        s.public_links.get(&link).is_some_and(|l| {
            l.app == app_id && (l.created_by == actor || s.manages_guests(&app, &actor))
        })
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
    Ok(Json(json!({})))
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
