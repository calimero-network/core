//! The API's routes for the app directory: instances, their metadata, access, transfers and moves.

#[allow(
    clippy::wildcard_imports,
    reason = "the handlers moved here from `api.rs` read its items"
)]
use super::*;

/// `GET /apps/{app}/attestation`'s query: only the nonce, since the app is the path's and
/// the purpose is always `receipts`. Other parameters are ignored, as they always were.
#[derive(Deserialize)]
pub(super) struct AppAttestQuery {
    nonce: String,
}

/// Every replica of an app attests its receipt key over `nonce`: what a user
/// checks to know the whole group holding the app is genuine, and which keys
/// sign its receipts.
async fn app_attestation(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
    Query(q): Query<AppAttestQuery>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    let app = node.app(&app_id).await?;
    let _p = gate(&node, &headers, &app, "*", false).await?;
    let mut replicas = Vec::new();
    for id in &app.replicas {
        let Some(addr) = node.sys(|s| s.nodes.get(id).map(|n| n.addr.clone())) else {
            continue;
        };
        let doc: Result<Value, String> = async {
            let mut url: url::Url = addr.parse().map_err(|e| format!("{e}"))?;
            url = url.join("attestation").map_err(|e| format!("{e}"))?;
            let _q = url
                .query_pairs_mut()
                .append_pair("nonce", &q.nonce)
                .append_pair("purpose", "receipts");
            node.http
                .get(url)
                .send()
                .await
                .and_then(reqwest::Response::error_for_status)
                .map_err(|e| e.to_string())?
                .json::<Value>()
                .await
                .map_err(|e| e.to_string())
        }
        .await;
        replicas.push(match doc {
            Ok(d) => json!({ "node": id, "attestation": d }),
            Err(e) => json!({ "node": id, "error": e }),
        });
    }
    // The cluster, said over the caller's attested channel: what each
    // replica's quote must bind, since the documents came in the clear.
    Ok(Json(
        json!({ "appId": app_id, "cluster": node.config.cluster, "replicas": replicas }),
    ))
}

/// `422 INIT_FAILED`: app `app_id`'s `init` failed with the app log's `error`.
pub(super) fn init_failed(app_id: &str, error: &Value) -> ApiError {
    ApiError::new(422, "INIT_FAILED", "the app's init failed")
        .with_details(json!({ "appId": app_id, "error": error }))
}

/// A release's service name to bundle hash map.
type Services = BTreeMap<String, String>;

/// The bundle an instance of `package` from deployment `id` runs: the
/// release the deployment offers for it (`version`, when given, must be that
/// release), service `service`. `404` for an unknown deployment; `400
/// NOT_OFFERED` for a package or version it does not offer. `CreateApp`
/// checks the offer again where it applies.
pub(crate) async fn offered_bundle(
    node: &Node,
    id: &str,
    package: &str,
    version: Option<&str>,
    service: Option<&str>,
) -> Result<(String, Option<String>, PackageRef), ApiError> {
    let find = || {
        node.sys(|s| {
            let d = s.deployments.get(id)?;
            let mut releases: Vec<(String, Services, Services)> = d
                .apps
                .values()
                .filter(|o| o.package == package && version.is_none_or(|v| v == o.version))
                .filter_map(|o| {
                    let r = s.offered_release(o)?;
                    Some((o.version.clone(), r.app_parts().clone(), r.compute.clone()))
                })
                .collect();
            releases.sort_by(|a, b| a.0.cmp(&b.0));
            releases.dedup_by(|a, b| a.0 == b.0);
            Some(releases)
        })
    };
    let releases = match find() {
        Some(r) => r,
        None => {
            let _caught_up = node.sys_fresh().await;
            find().ok_or_else(|| ApiError::not_found("no such deployment"))?
        }
    };
    let (version, services, compute) = match releases.as_slice() {
        [] => {
            return Err(ApiError::bad_request(
                "NOT_OFFERED",
                &format!(
                    "the deployment does not offer {package}{}",
                    version.map(|v| format!("@{v}")).unwrap_or_default()
                ),
            ))
        }
        [one] => one,
        _ => {
            return Err(ApiError::bad_request(
                "VERSION_REQUIRED",
                &format!("the deployment offers several releases of {package}; name one"),
            ))
        }
    };
    let key = service.unwrap_or_default();
    let release = PackageRef {
        package: package.to_owned(),
        version: version.clone(),
        service: (!key.is_empty()).then(|| key.to_owned()),
        compute: false,
    };
    let found = services
        .get(key)
        .map(|b| (b.clone(), compute.get(key).cloned(), release));
    found.ok_or_else(|| {
        ApiError::bad_request(
            "NO_SUCH_SERVICE",
            &format!("{package}@{version} has no service {key:?}"),
        )
    })
}

/// The organization an instance that names none goes to before organizations are
/// ordinary (below the cluster version that brings `Feature::OrdinaryOrgs`): `owner`'s
/// personal organization ([`crate::types::Org::personal`]), made now if they have none.
async fn personal_org(node: &Node, owner: &str) -> Result<String, ApiError> {
    let find = || node.sys(|s| s.personal_org_of(owner).cloned());
    if let Some(org) = find() {
        return Ok(org);
    }
    let _caught_up = node.sys_fresh().await;
    if let Some(org) = find() {
        return Ok(org);
    }
    let org_id = typed_id::<16>();
    let made = node
        .sys_write(&SystemCmd::CreateOrg {
            org_id: org_id.clone(),
            name: crate::system::PERSONAL_ORG_NAME.to_owned(),
            actor: owner.to_owned(),
            release: Some(node.own_release()),
            recovery_key: None,
            at: recovery_now(node),
            name_fits: true,
            personal: true,
            app_team: None,
            capabilities: None,
        })
        .await;
    match made {
        Ok(_) => Ok(org_id),
        // Another request of the same account made it first: use that one.
        Err(e) if e.code == crate::node::PERSONAL_ORG_EXISTS => {
            let _caught_up = node.sys_fresh().await;
            find().ok_or_else(|| {
                ApiError::unavailable("the personal organization is not readable yet")
            })
        }
        Err(e) => Err(e),
    }
}

/// `400 ORG_REQUIRED`: the creator is in several organizations and named none.
pub(crate) const ORG_REQUIRED: &str = "ORG_REQUIRED";
/// `409 NO_ORGANIZATION`: an `org` instance, and its creator is in no organization.
pub(crate) const NO_ORGANIZATION: &str = "NO_ORGANIZATION";
/// `400 STANDALONE_INSTANCE`: a `standalone` instance names an existing organization.
pub(crate) const STANDALONE_INSTANCE: &str = "STANDALONE_INSTANCE";
/// `400 ORG_INSTANCE`: an `org` instance asks for a new organization.
pub(crate) const ORG_INSTANCE: &str = "ORG_INSTANCE";

/// The instance mode of `release` (`instanceOrg` in its signed manifest): the one its
/// record keeps, else (a release recorded before the node kept it) its `.mpk`'s. A raw
/// bundle, or a release no longer installed, is `either`.
pub(crate) async fn instance_org_of(
    node: &Node,
    release: Option<&PackageRef>,
) -> Result<calimero_bundle::InstanceOrg, ApiError> {
    let Some(r) = release else {
        return Ok(calimero_bundle::InstanceOrg::Either);
    };
    let Some(v) = node.sys(|s| {
        s.packages
            .get(&r.package)
            .and_then(|p| p.versions.get(&r.version))
            .cloned()
    }) else {
        return Ok(calimero_bundle::InstanceOrg::Either);
    };
    if let Some(mode) = v.instance_org {
        return Ok(mode);
    }
    crate::consent_page::declared_instance_org(node, &r.package, &v).await
}

/// The name an organization made with an instance gets: the one the request gives, else
/// the instance's own name made to fit ([`crate::system::org_name_fits`]: one line of 1 to
/// 64 characters), else the package's id, else `Instance`.
fn standalone_org_name(
    asked: Option<&NewOrgBody>,
    instance: &str,
    release: Option<&PackageRef>,
) -> String {
    if let Some(n) = asked {
        return n.name.clone();
    }
    let fit = |name: &str| -> Option<String> {
        let one_line: String = name
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect();
        let cut: String = one_line
            .trim()
            .chars()
            .take(crate::system::MAX_ORG_NAME_CHARS)
            .collect();
        let cut = cut.trim().to_owned();
        (!cut.is_empty()).then_some(cut)
    };
    fit(instance)
        .or_else(|| release.and_then(|r| fit(&r.package)))
        .unwrap_or_else(|| "Instance".to_owned())
}

/// The organization a new instance (not a unit's member, which joins its instance's)
/// goes into, judged by its release's instance mode once organizations are ordinary
/// (`Feature::OrdinaryOrgs`):
/// - `standalone`: a new organization, made now (its creator the owner); naming one is
///   `400 STANDALONE_INSTANCE`.
/// - `org`: the one named, else the creator's only one (`member` or above); several is `400
///   ORG_REQUIRED`, none `409 NO_ORGANIZATION`; asking for a new one is `400 ORG_INSTANCE`.
/// - `either`: the one named, a new one when asked for (`newOrg`), else the creator's only
///   one, else (none) a new one; several is `400 ORG_REQUIRED`.
///
/// Below that version, as before: the one named, else the owner's personal organization.
async fn instance_org(
    node: &Node,
    owner: &str,
    body: &CreateAppBody,
    release: Option<&PackageRef>,
) -> Result<String, ApiError> {
    use crate::system::cluster_version::Feature;
    use calimero_bundle::InstanceOrg;
    if body.org.is_some() && body.new_org.is_some() {
        return Err(ApiError::bad_request(
            "BAD_REQUEST",
            "name an organization (`org`) or ask for a new one (`newOrg`), not both",
        ));
    }
    if node
        .require_features(&[Feature::OrdinaryOrgs])
        .await
        .is_err()
    {
        if body.new_org.is_some() {
            node.require_features(&[Feature::OrdinaryOrgs]).await?;
        }
        return match &body.org {
            Some(org) => Ok(org.clone()),
            None => personal_org(node, owner).await,
        };
    }
    let mode = instance_org_of(node, release).await?;
    let label = release.map_or_else(|| "this bundle".to_owned(), |r| r.package.clone());
    if let Some(org) = &body.org {
        if mode == InstanceOrg::Standalone {
            return Err(ApiError::bad_request(
                STANDALONE_INSTANCE,
                &format!("an instance of {label} comes with an organization of its own: name none"),
            ));
        }
        return Ok(org.clone());
    }
    match (mode, &body.new_org) {
        (InstanceOrg::Org, Some(_)) => {
            return Err(ApiError::bad_request(
                ORG_INSTANCE,
                &format!(
                    "an instance of {label} goes into an organization you are in; it makes none"
                ),
            ))
        }
        (InstanceOrg::Standalone, _) | (InstanceOrg::Either, Some(_)) => {}
        (InstanceOrg::Org | InstanceOrg::Either, None) => {
            let _caught_up = node.sys_fresh().await;
            let mine: Vec<String> = node.sys(|s| s.orgs_of(owner).into_iter().cloned().collect());
            match mine.as_slice() {
                [one] => return Ok(one.clone()),
                [] if mode == InstanceOrg::Org => {
                    return Err(ApiError::new(
                        409,
                        NO_ORGANIZATION,
                        &format!("an instance of {label} goes into an organization you are in, and you are in none: create one or accept an invite first"),
                    ))
                }
                [] => {}
                _ => {
                    return Err(ApiError::bad_request(
                        ORG_REQUIRED,
                        "you are in several organizations: name the one the instance goes into (`org`)",
                    ))
                }
            }
        }
    }
    // A new organization, its creator the owner, approving this node's release as the
    // personal one did.
    let org_id = typed_id::<16>();
    let _made = node
        .sys_write(&SystemCmd::CreateOrg {
            org_id: org_id.clone(),
            name: standalone_org_name(body.new_org.as_ref(), &body.name, release),
            actor: owner.to_owned(),
            release: Some(node.own_release()),
            recovery_key: None,
            at: recovery_now(node),
            name_fits: true,
            personal: false,
            app_team: None,
            capabilities: None,
        })
        .await?;
    Ok(org_id)
}

/// `newOrg` in `POST /apps`: the organization made with the instance.
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct NewOrgBody {
    /// Its name ([`crate::system::org_name_fits`]).
    name: String,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct CreateAppBody {
    #[serde(default)]
    name: String,
    /// A bundle hash (a raw upload), or `package` (+ `version`, `service`).
    #[serde(default)]
    bundle: Option<String>,
    #[serde(default)]
    package: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    service: Option<String>,
    #[serde(default)]
    org: Option<String>,
    /// A new organization for the instance, made with it (its creator the owner), for a
    /// release whose instances may go into a new one (`instanceOrg`: `standalone` or
    /// `either`). Not with `org`.
    #[serde(default)]
    new_org: Option<NewOrgBody>,
    /// Who reaches the instance, as grants (`types::Grant`): the organization or teams of
    /// it, named members and guests (each with an app-defined `scope`), anyone. Absent: the
    /// organization; empty: the owner alone.
    #[serde(default)]
    grants: Option<Vec<crate::types::Grant>>,
    /// Arguments for the app's `init`, as JSON.
    #[serde(default)]
    init_args: Value,
    /// The instance's metadata.
    #[serde(default)]
    metadata: Value,
    /// The deployment to create it from: then `package` names one of the
    /// packages it offers (and `version`, if given, the offered release),
    /// and the instance runs exactly the release it offers.
    #[serde(default)]
    deployment: Option<String>,
    /// For a unit's member created from its instance's deployment: the release the
    /// deployment offers for the instance, which covers the member (`CreateApp {
    /// unit_offer }`). The node's own, never a client's.
    #[serde(skip)]
    unit_offer: Option<PackageRef>,
    /// Asked by a session bound to the package's own frontend through the consent page's
    /// install sheet: it runs a release that frontend sees, as `POST /apps` from that
    /// session does. The node's own, never a client's.
    #[serde(skip)]
    from_frontend: bool,
}

/// What the consent page's install sheet creates (`install`), in `POST /apps`'s terms.
#[derive(Clone, Debug)]
pub(crate) struct Install {
    pub name: String,
    pub package: String,
    pub version: Option<String>,
    pub deployment: Option<String>,
    pub org: Option<String>,
    pub new_org: Option<String>,
    pub init_args: Value,
    /// Asked from the package's own frontend (a session bound to it).
    pub from_frontend: bool,
}

/// Create the instance an install sheet showed, for `owner`, as `POST /apps` does.
pub(crate) async fn create_installed(
    node: &Arc<Node>,
    p: &Principal,
    owner: String,
    i: Install,
) -> Result<Value, ApiError> {
    let body = CreateAppBody {
        name: i.name,
        bundle: None,
        package: Some(i.package),
        version: i.version,
        service: None,
        org: i.org,
        new_org: i.new_org.map(|name| NewOrgBody { name }),
        grants: None,
        init_args: i.init_args,
        metadata: Value::Null,
        deployment: i.deployment,
        unit_offer: None,
        from_frontend: i.from_frontend,
    };
    create_instance(node, p, owner, body, 0).await
}

async fn app_create(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> ApiResult {
    let raw: Value = serde_json::from_slice(&body)
        .map_err(|e| ApiError::bad_request("BAD_REQUEST", &e.to_string()))?;
    // The single access mode clients before 1.0.0-rc.33 sent is refused, never ignored:
    // read as no grants it would admit the whole organization to an instance the client
    // meant to keep to a few. Judged on the raw body, so `CreateAppBody` (and the API
    // document clients are generated from) never names it.
    if raw.get("access").is_some() {
        return Err(ApiError::bad_request(
            "ACCESS_REMOVED",
            "`access` was replaced by `grants`: name who reaches the instance as a list of grants",
        ));
    }
    let body: CreateAppBody = serde_json::from_value(raw)
        .map_err(|e| ApiError::bad_request("BAD_REQUEST", &e.to_string()))?;
    if let Some(grants) = &body.grants {
        grants_fit(grants)?;
    }
    let p = signed_in(&node, &headers).await?;
    match p.bound_app() {
        // A frontend makes instances of its own package, already installed.
        Some(pkg) => {
            let installed = node.sys(|s| s.packages.contains_key(pkg));
            if body.bundle.is_some() || body.package.as_deref() != Some(pkg) || !installed {
                return Err(ApiError::forbidden(
                    "a session opened on an app's frontend creates instances of that app only",
                ));
            }
        }
        None => {
            let _p = installer(&node, &headers).await?;
        }
    }
    if p.api_key.as_ref().is_some_and(|k| k.read_only) {
        return Err(ApiError::forbidden("read-only API key"));
    }
    let owner = p.account()?;
    create_instance(&node, &p, owner, body, 0).await.map(Json)
}

/// Refused `403 DEPLOY_NOT_GRANTED` when `owner` may not create an instance from no
/// deployment ([`crate::system::SystemState::creates_outside_offers`]); else whether its
/// `CreateApp` carries the rule, once every node reads it.
async fn outside_offers(
    node: &Node,
    owner: &str,
    release: Option<&PackageRef>,
    now: u64,
) -> Result<bool, ApiError> {
    let may = || node.sys(|s| s.creates_outside_offers(owner, release, now));
    if !may() {
        let _caught_up = node.sys_fresh().await;
        if !may() {
            return Err(crate::node::not_a_deployer());
        }
    }
    let granted = node.sys(|s| s.governance().deployments == crate::system::Deployments::Granted);
    Ok(granted
        && node
            .require_features(&[crate::system::cluster_version::Feature::OffersOnly])
            .await
            .is_ok())
}

/// How deep `create` references may nest: an app, the apps it creates, theirs.
pub(crate) const MAX_UNIT_DEPTH: usize = 3;

/// Creates one instance for `owner`, after the apps its release creates with it (`uses`
/// with `mode: create`, [`unit_members`]), and waits for its `init`. `depth` counts the
/// instances above this one in its unit.
async fn create_instance(
    node: &Arc<Node>,
    p: &Principal,
    owner: String,
    body: CreateAppBody,
    depth: usize,
) -> Result<Value, ApiError> {
    let now = now_secs();
    let (bundle, compute_bundle, release) = match (&body.deployment, &body.unit_offer) {
        // From a deployment's offer: installed already, and the caller's
        // to run whether or not they see the release otherwise.
        (Some(d), None) => {
            if body.bundle.is_some() {
                return Err(ApiError::bad_request(
                    "BAD_REQUEST",
                    "an instance from a deployment names its package, not a bundle",
                ));
            }
            let package = body.package.as_deref().ok_or_else(|| {
                ApiError::bad_request("BAD_REQUEST", "name the package the deployment offers")
            })?;
            let (bundle, compute, release) = offered_bundle(
                node,
                d,
                package,
                body.version.as_deref(),
                body.service.as_deref(),
            )
            .await?;
            (bundle, compute, Some(release))
        }
        // A unit's member, or an app from a release or a bundle.
        _ => {
            bundle_named(
                node,
                body.bundle.clone(),
                body.package.as_deref(),
                body.version.as_deref(),
                body.service.as_deref(),
                &owner,
                // Checked above: a bound session names its own package only. A member from
                // a deployment runs the release its instance's offered release pins, whether
                // or not its owner sees it otherwise, as the instance does.
                p.bound_app().is_some() || body.unit_offer.is_some() || body.from_frontend,
            )
            .await?
        }
    };
    let offers_only = match body.deployment {
        None => outside_offers(node, &owner, release.as_ref(), now).await?,
        Some(_) => false,
    };
    // The executor is decided once, here, from the WASM the bundle names (a
    // package's service, or the raw upload), and recorded with the app. A core
    // module (an SDK 0.12 app) makes no app.
    let executor = node.executor_of(&bundle).await?;
    if executor != crate::types::Executor::Component {
        return Err(ApiError::unsupported_bundle());
    }
    // A component's confirm methods are its ABI's (S5), never the request's.
    if serde_json::to_vec(&body.init_args).map_or(0, |a| a.len()) > crate::node::MAX_ARGS_BYTES {
        return Err(crate::node::args_too_large());
    }
    let confirm_methods: Vec<String> = Vec::new();
    // Every instance is in an organization: a unit's member joins its instance's, any
    // other goes where its release's instance mode says (`instance_org`).
    let org = match (&body.org, depth) {
        (Some(org), 1..) => org.clone(),
        _ => instance_org(node, &owner, &body, release.as_ref()).await?,
    };
    // Past a release deadline the app's approver has not met, no new app;
    // `CreateApp` asks again at the same time.
    if let Some(stall) = node.sys(|s| {
        let governing = s.governing(release.as_ref(), &bundle, now);
        s.stalled_for_release(governing, &org, now)
    }) {
        return Err(crate::node::stalled(&stall));
    }
    // Only nodes on an approved release (the publisher's, the owner's for an
    // unbound package, or the operator's where the deployment gave it
    // release approvals) may hold the app.
    let approved = node.sys(|s| {
        let governing = s.governing(release.as_ref(), &bundle, now);
        s.approved_for_release(governing, &org).cloned()
    });
    // Nor a node whose runtime is older than the release needs.
    let min_runtime = node.min_runtime_of(release.as_ref(), &bundle).await;
    let replicas = node
        .place(approved.as_ref(), now, min_runtime.as_ref())
        .await;
    if replicas.is_empty() {
        if let Some(min) = &min_runtime {
            let nodes = node.sys(|s| s.replica_nodes().map(|n| n.id).collect::<Vec<_>>());
            let short = crate::runtime::short_of(&nodes, &node.runtimes(&nodes).await, min);
            if !short.is_empty() {
                return Err(crate::runtime::refusal(
                    &release_label(release.as_ref(), &bundle),
                    min,
                    short,
                ));
            }
        }
        return Err(ApiError::unavailable(
            "no node runs a release this app's owner approved",
        ));
    }
    // The apps its release creates with it, made first in the same organization, so its
    // `init` can be told their ids.
    let mut init_args = body.init_args;
    let links = match &release {
        Some(release) => {
            let unit = unit_members(node, release).await?;
            if unit.is_empty() {
                BTreeMap::new()
            } else {
                if depth >= MAX_UNIT_DEPTH {
                    return Err(ApiError::bad_request(
                        "UNIT_TOO_DEEP",
                        &format!("apps a release creates nest at most {MAX_UNIT_DEPTH} deep"),
                    ));
                }
                // Members of an instance from a deployment carry the deployment.
                let mut features = vec![crate::system::cluster_version::Feature::AppLinks];
                if body.deployment.is_some() {
                    features.push(crate::system::cluster_version::Feature::WholeUnits);
                }
                node.require_features(&features).await?;
                if init_args.get("links").is_some() {
                    return Err(ApiError::bad_request(
                        "BAD_REQUEST",
                        "`links` in initArgs is the node's: it names the apps the release creates",
                    ));
                }
                // The deployment's offer covering the whole unit: the instance's own release
                // at its top, handed down to the members' members.
                let unit_offer = body.deployment.as_ref().map(|_| {
                    body.unit_offer.clone().unwrap_or_else(|| PackageRef {
                        package: release.package.clone(),
                        version: release.version.clone(),
                        service: None,
                        compute: false,
                    })
                });
                let mut links = BTreeMap::new();
                for (name, (package, version)) in unit.members {
                    let member = CreateAppBody {
                        name: format!("{}.{name}", body.name),
                        bundle: None,
                        package: Some(package),
                        version: Some(version),
                        service: None,
                        org: Some(org.clone()),
                        new_org: None,
                        grants: body.grants.clone(),
                        init_args: Value::Null,
                        metadata: json!({ "unit": { "of": body.name, "as": name } }),
                        deployment: body.deployment.clone(),
                        unit_offer: unit_offer.clone(),
                        from_frontend: body.from_frontend,
                    };
                    let made = Box::pin(create_instance(node, p, owner.clone(), member, depth + 1))
                        .await?;
                    let id = made["appId"].as_str().unwrap_or_default().to_owned();
                    let _prev = links.insert(name, id);
                }
                if unit.init_takes_links {
                    if !init_args.is_object() {
                        init_args = json!({});
                    }
                    init_args["links"] = json!(links);
                }
                links
            }
        }
        None => BTreeMap::new(),
    };
    let links_answer = links.clone();
    let app_id = typed_id::<16>();
    let has_compute = compute_bundle.is_some();
    let components = node.sys(|s| s.components_of(release.as_ref()));
    let access = body.grants.map_or_else(Access::org, Access::new);
    let out = node
        .sys_write(&SystemCmd::CreateApp {
            app: Box::new(crate::types::AppRecord {
                app_id: app_id.clone(),
                name: body.name,
                group: 0,
                bundle,
                owner,
                org,
                access,
                confirm_methods,
                executor,
                replicas,
                created_at: now,
                init_args,
                metadata: body.metadata,
                last_upgrade: None,
                deployment: body.deployment,
                compute_bundle,
                components,
                release,
                restore: None,
                links,
                attached: BTreeMap::new(),
                restore_point: None,
                egress_off: false,
            }),
            unit_offer: body.unit_offer,
            offers_only,
        })
        .await?;
    // Wait for the app's first entry (init) to commit somewhere; a compute-only app's is empty,
    // but its group cannot take a pipeline start before it.
    let deadline = tokio::time::Instant::now() + node.app_start_wait();
    loop {
        match app_ready(node, &app_id).await {
            Some(Ok(())) => break,
            Some(Err(error)) => {
                return Err(init_failed(&app_id, &error));
            }
            None => {}
        }
        if tokio::time::Instant::now() > deadline {
            return Err(ApiError::unavailable(
                "the app was created but has not started yet",
            ));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Ok(json!({
        "appId": app_id,
        "contextId": hex::encode(sha256(app_id.as_bytes())),
        "group": out["group"],
        "replicas": out["replicas"],
        "compute": has_compute,
        "links": links_answer,
    }))
}

/// What a release creates with an instance of it: by name, the package and the release of
/// each `create` reference in its signed manifest (`uses`), and whether its `init` takes
/// `links` (then the node passes it the ids).
pub(crate) struct Unit {
    pub members: BTreeMap<String, (String, String)>,
    pub init_takes_links: bool,
}

impl Unit {
    pub(crate) fn is_empty(&self) -> bool {
        self.members.is_empty()
    }
}

/// The [`Unit`] of `release`, read from its verified manifest and embedded ABI. Each
/// member's release is the one its lock pinned (`uses.version`), else the newest installed
/// release of the package inside its range; a package with none is `409
/// USES_NOT_INSTALLED`, naming what to install.
pub(crate) async fn unit_members(
    node: &Node,
    release: &crate::system::PackageRef,
) -> Result<Unit, ApiError> {
    let Some(version) = node.sys(|s| {
        s.packages
            .get(&release.package)
            .and_then(|p| p.versions.get(&release.version))
            .cloned()
    }) else {
        return Ok(Unit {
            members: BTreeMap::new(),
            init_takes_links: false,
        });
    };
    let card = crate::consent_page::release_capabilities(
        node,
        &release.package,
        &version,
        release.service.as_deref(),
    )
    .await?;
    // The card is the node's own (`release_capabilities`); a `uses` it holds
    // that does not read would create the instance without its unit.
    let uses: BTreeMap<String, calimero_bundle::BundleUse> = match &card["uses"] {
        Value::Null => BTreeMap::new(),
        v => serde_json::from_value(v.clone())
            .map_err(|e| ApiError::internal(&format!("the release's uses do not read: {e}")))?,
    };
    let init_takes_links = card["abi"]["methods"]["init"]["params"]
        .as_array()
        .is_some_and(|ps| ps.iter().any(|p| p[0] == "links"));
    let mut members = BTreeMap::new();
    for (name, u) in uses {
        if u.mode != calimero_bundle::UseMode::Create {
            continue;
        }
        if let Err(e) = semver::VersionReq::parse(&u.range) {
            return Err(ApiError::bad_request("BAD_USES", &e.to_string()));
        }
        let chosen = node.sys(|s| s.release_of_use(&(&u).into()).map(|(_, v)| v));
        let Some(chosen) = chosen else {
            return Err(ApiError::new(
                409,
                "USES_NOT_INSTALLED",
                &format!(
                    "{}@{} creates {name} from {} {}, which is not installed here: install {}",
                    release.package,
                    release.version,
                    u.package,
                    u.version.as_deref().unwrap_or(&u.range),
                    u.package,
                ),
            ));
        };
        let _prev = members.insert(name, (u.package, chosen));
    }
    Ok(Unit {
        members,
        init_takes_links,
    })
}

/// How long creating an app waits for its `init` to commit before answering `503 UNAVAILABLE`
/// (the app stays created, and `init` keeps being retried).
pub(crate) const APP_START_WAIT: Duration = Duration::from_secs(30);

/// Whether the app has run `init`, asking a replica if this node hosts none.
/// Whether the app's init has committed: `None` not yet, `Some(Err(why))` failed.
pub async fn app_ready(node: &Node, app_id: &str) -> Option<Result<(), Value>> {
    if let Some(g) = node.hosted(app_id) {
        return g.read_local(|m, _| init_state(&m.state));
    }
    let app = node.app(app_id).await.ok()?;
    let addr = node.sys(|s| {
        app.replicas
            .iter()
            .find_map(|r| s.nodes.get(r).map(|n| n.addr.clone()))
    })?;
    let v = node
        .peer_forward(&addr, "internal/app-ready", &json!({ "appId": app_id }))
        .await
        .ok()?;
    match (v.get("ready").and_then(Value::as_bool), v.get("initError")) {
        (_, Some(e)) if !e.is_null() => Some(Err(e.clone())),
        (Some(true), _) => Some(Ok(())),
        _ => None,
    }
}

#[derive(Deserialize)]
pub(super) struct AppListQuery {
    /// Only instances of this package (any version).
    #[serde(default)]
    package: Option<String>,
    /// Only instances running this bundle.
    #[serde(default)]
    bundle: Option<String>,
}

async fn app_list(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Query(q): Query<AppListQuery>,
) -> ApiResult {
    let p = principal(&node, &headers).await?;
    let account = p
        .as_ref()
        .and_then(|p| p.caller.account().map(str::to_owned));
    let bound = p.as_ref().and_then(|p| p.bound_app().map(str::to_owned));
    let key = p.as_ref().and_then(|p| p.api_key.clone());
    let apps: Vec<Value> = node.sys(|s| {
        s.apps
            .values()
            .filter(|a| key.as_ref().is_none_or(|k| super::key_scoped_to(k, &a.app_id)))
            .map(|a| (a, s.running_release(a)))
            .filter(|(_, r)| {
                bound
                    .as_ref()
                    .is_none_or(|pkg| r.as_ref().is_some_and(|r| r.package == *pkg))
            })
            .filter(|(a, _)| !a.access.anyone() || account.as_deref() == Some(a.owner.as_str()))
            .filter(|(a, _)| s.may_reach(a, account.as_deref()))
            .filter(|(a, _)| q.bundle.as_ref().is_none_or(|b| *b == a.bundle))
            .filter(|(_, r)| {
                q.package
                    .as_ref()
                    .is_none_or(|p| r.as_ref().is_some_and(|r| r.package == *p))
            })
            .map(|(a, release)| {
                json!({
                    "appId": a.app_id,
                    "name": a.name,
                    "owner": a.owner,
                    "org": a.org,
                    "bundle": a.bundle,
                    "package": release.as_ref().map(|r| &r.package),
                    "version": release.as_ref().map(|r| &r.version),
                    "deployment": a.deployment,
                })
            })
            .collect()
    });
    Ok(Json(json!({ "apps": apps })))
}

/// Delete an app instance: its owner, or an admin of its organization. An instance whose
/// release created apps with it goes with its whole unit (`members`); a unit's member is not
/// deleted alone (`409 UNIT_MEMBER`).
async fn app_delete(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    let p = holder(&node, &headers).await?;
    let account = p.account()?;
    let app = node.app(&app_id).await?;
    p.within(&node, &app)?;
    let (instance, unit, logged) = node.sys(|s| {
        (
            s.unit_parent(&app_id).map(str::to_owned),
            s.unit_of(&app_id),
            s.cluster_version.clone(),
        )
    });
    if let Some(instance) = instance {
        return Err(ApiError::new(
            409,
            "UNIT_MEMBER",
            &format!("this app is a member of instance {instance}'s unit: delete the instance"),
        ));
    }
    // The whole-unit rule once the cluster applies it; an instance with members waits for it
    // (`409 FEATURE_NOT_ACTIVE`), and any other app is deleted alone either way.
    let whole = !unit.is_empty()
        || crate::system::cluster_version::check(
            logged.as_deref(),
            &[crate::system::cluster_version::Feature::WholeUnits],
        )
        .is_ok();
    let _ok = node
        .sys_write(&SystemCmd::DeleteApp {
            app_id: app_id.clone(),
            actor: account,
            owner_in_org: true,
            owner_in_team: true,
            members: whole.then(|| unit.clone()),
        })
        .await?;
    Ok(Json(
        json!({ "appId": app_id, "deleted": true, "members": unit }),
    ))
}

async fn app_get(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    let p = principal(&node, &headers).await?;
    // As in `gate`: an anonymous caller is judged on the cluster's latest
    // system state too, so an app made private through another node is hidden
    // here at once (#4723). `principal` already read fresh for a credential.
    // Read before the record, so the record answered is the fresh one.
    if p.is_none() {
        node.sys_fresh().await;
    }
    let app = node.app(&app_id).await?;
    if let Some(p) = &p {
        p.within(&node, &app)?;
    }
    let account = p.as_ref().and_then(|p| p.caller.account());
    if !reaches(&node, &app, account) {
        return Err(ApiError::not_found("no such app"));
    }
    // Its public links are its managers' to see.
    let manages = account.is_some_and(|a| node.sys(|s| s.manages_grants(&app, a)));
    let mut app = app;
    if !manages {
        app.access = app.access.without_links();
    }
    let mut v = serde_json::to_value(&app).unwrap_or_default();
    // A guest sees none of the organization's people: only their own grant.
    if let Some(a) = account {
        if node.sys(|s| s.is_guest(&app, a)) {
            v["grants"] = json!(app.access.only(a));
        }
    }
    Ok(Json(v))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct AppInviteBody {
    /// How long it lasts (default 7 days, at most `MAX_APP_INVITE_TTL_SECS`).
    #[serde(default)]
    ttl_secs: Option<u64>,
    /// How many accounts it admits (default 1, at most `MAX_APP_INVITE_USES`).
    #[serde(default)]
    uses: Option<u32>,
    /// What the grant it makes lets its redeemer do inside the app, in the app's own terms
    /// (`Grant::Account`'s `scope`): opaque JSON of at most `MAX_GRANT_SCOPE_BYTES`, which the
    /// app reads for its caller. Absent: the app's whole.
    #[serde(default)]
    scope: Option<Value>,
    #[serde(default)]
    confirm: Option<ConfirmBody>,
}

/// The instance's switches, judged before an invite is minted: who may, and that it takes
/// guests. `404` to whoever does not reach it.
fn refuse_app_invite(
    node: &Node,
    app: &crate::types::AppRecord,
    actor: &str,
) -> Result<(), ApiError> {
    node.sys(|s| {
        if !s.may_reach(app, Some(actor)) {
            return Err(ApiError::not_found("no such app"));
        }
        if !s.manages_grants(app, actor) {
            return Err(ApiError::forbidden(
                "only whoever may change the instance's access invites its guests",
            ));
        }
        Ok(())
    })
}

/// Mint an instance invite: whoever manages the instance's guests, with their passkey over
/// the invite's key and terms (`412` first). The first request makes the token and keeps it
/// with the challenge; the log records only the token's public key.
async fn app_invite_create(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
    Json(body): Json<AppInviteBody>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    let p = holder(&node, &headers).await?;
    let actor = p.account()?;
    node.sys_fresh_from(p.sys_index).await;
    let app = node.app(&app_id).await?;
    refuse_app_invite(&node, &app, &actor)?;
    let ttl = body.ttl_secs.unwrap_or(7 * 24 * 3600);
    let uses = body.uses.unwrap_or(1);
    if ttl == 0 || ttl > crate::system::MAX_APP_INVITE_TTL_SECS {
        return Err(ApiError::bad_request(
            "BAD_REQUEST",
            "ttlSecs is 1 to 7776000 (90 days)",
        ));
    }
    if uses == 0 || uses > crate::system::MAX_APP_INVITE_USES {
        return Err(ApiError::bad_request("BAD_REQUEST", "uses is 1 to 1000"));
    }
    if body
        .scope
        .as_ref()
        .is_some_and(|v| !crate::types::scope_fits(v))
    {
        return Err(ApiError::bad_request(
            "BAD_REQUEST",
            "scope is at most 256 bytes of JSON",
        ));
    }
    node.require_features(&[
        crate::system::cluster_version::Feature::Guests,
        crate::system::cluster_version::Feature::AccessGrants,
    ])
    .await?;
    let now = now_secs();
    let token = format!("inv_{}", b64url(&random::<24>()));
    let invite = crate::types::AppInvite {
        app: app_id.clone(),
        expires_at: now + ttl,
        uses_left: uses,
        created_by: actor.clone(),
        created_at: now,
        scope: body.scope,
    };
    let args = crate::system::app_invite_args(&crate::proof::invite_key(&token), &invite);
    let c = consent_minted(
        &node,
        &p,
        &app_id,
        "__create_app_invite",
        &args,
        Some(json!(token)),
        body.confirm,
    )
    .await?;
    // The retry records the invite the approval names, minted by the first request.
    let token = c
        .carry
        .as_ref()
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| ApiError::bad_request("WRONG_CHALLENGE", "not an invite approval"))?;
    let invite_key = crate::proof::invite_key(&token);
    let expires_at = c
        .args
        .get("expiresAt")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let invite = crate::types::AppInvite {
        expires_at,
        created_at: expires_at.saturating_sub(ttl),
        ..invite
    };
    if crate::system::app_invite_args(&invite_key, &invite) != c.args {
        return Err(ApiError::bad_request(
            "WRONG_CHALLENGE",
            "the approval was for a different call",
        ));
    }
    let _ok = node
        .sys_write(&SystemCmd::CreateAppInvite {
            invite_key,
            invite,
            actor,
            consent: c.consent,
        })
        .await?;
    Ok(Json(json!({ "token": token })))
}

/// The instance's pending invites, for whoever manages its guests: key, uses left, expiry,
/// creator; never a token (the log never had one). `404` to anyone else.
async fn app_invite_list(
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
        if !s.may_reach(&app, Some(&actor)) || !s.manages_grants(&app, &actor) {
            return Err(ApiError::not_found("no such app"));
        }
        let invites: Vec<Value> = s
            .app_invites
            .iter()
            .filter(|(_, i)| i.app == app_id)
            .map(|(k, i)| {
                json!({
                    "inviteKey": k,
                    "usesLeft": i.uses_left,
                    "expiresAt": i.expires_at,
                    "expired": i.expires_at <= now,
                    "createdBy": i.created_by,
                    "createdAt": i.created_at,
                    "scope": i.scope,
                })
            })
            .collect();
        Ok(Json(json!({ "invites": invites })))
    })
}

/// Withdraw an instance invite: its creator or whoever manages the instance's guests, with
/// their passkey (`412` first). One the caller may not see is `404`.
async fn app_invite_revoke(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((app_id, key)): Path<(String, String)>,
    body: Option<Json<ConfirmOnlyBody>>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    let p = holder(&node, &headers).await?;
    let actor = p.account()?;
    node.sys_fresh_from(p.sys_index).await;
    let app = node.app(&app_id).await?;
    let visible = node.sys(|s| {
        s.app_invites.get(&key).is_some_and(|i| {
            i.app == app_id && (i.created_by == actor || s.manages_grants(&app, &actor))
        })
    });
    if !visible {
        return Err(ApiError::not_found(crate::system::APP_INVITE_GONE));
    }
    let args = json!({ "inviteKey": key });
    let consent = consent(
        &node,
        &p,
        &app_id,
        "__revoke_app_invite",
        &args,
        body.and_then(|Json(b)| b.confirm),
    )
    .await?;
    let _ok = node
        .sys_write(&SystemCmd::RevokeAppInvite {
            app_id,
            invite_key: key,
            actor,
            consent,
        })
        .await?;
    Ok(Json(json!({})))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct AccessBody {
    /// Who reaches the instance, as grants (`types::Grant`); empty: the owner alone.
    grants: Vec<crate::types::Grant>,
}

/// `400` for a grant list larger than an instance holds, before it is proposed; the rest is
/// judged where it applies (`SystemState::check_grants`).
fn grants_fit(grants: &[crate::types::Grant]) -> Result<(), ApiError> {
    if grants.len() > crate::types::MAX_GRANTS {
        return Err(ApiError::bad_request(
            "BAD_REQUEST",
            "an instance holds at most 1018 grants",
        ));
    }
    Ok(())
}

async fn app_access(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
    Json(body): Json<AccessBody>,
) -> ApiResult {
    grants_fit(&body.grants)?;
    let access = Access::new(body.grants);
    let app_id = node.canonical_app_id(&app_id);
    let p = person(&node, &headers).await?;
    let account = p.account()?;
    let app = node.app(&app_id).await?;
    p.within(&node, &app)?;
    // A package-bound session moves its package's app between its package's
    // teams only (a channel made private, or back to the app team).
    match (&app.org, app.access.grants(), access.grants()) {
        (
            org,
            [crate::types::Grant::Team { team: from }],
            [crate::types::Grant::Team { team: to }],
        ) => {
            p.within_team(&node, org, from, OwnTeam::AppTeamOrBelow)?;
            p.within_team(&node, org, to, OwnTeam::AppTeamOrBelow)?;
        }
        _ => p.unbound()?,
    }
    let _ok = node
        .sys_write(&SystemCmd::SetAccess {
            app_id,
            access,
            actor: account,
            owner_in_org: true,
            owner_in_team: true,
        })
        .await?;
    Ok(Json(json!({})))
}

/// Delete an organization, its teams, invites and apps: the owner's, with a
/// passkey assertion over this exact deletion (a 412 asks for it).
#[derive(Deserialize)]
pub(super) struct TransferBody {
    to: String,
    #[serde(default)]
    confirm: Option<ConfirmBody>,
}

/// Hand an app to another account: the owner's (of an organization's app,
/// while a member of it) or an organization admin's, with a passkey assertion
/// over this transfer (412 first). Who then upgrades the app, and so which
/// code reads its data, is the new owner's.
async fn app_transfer(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
    Json(body): Json<TransferBody>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    let p = holder(&node, &headers).await?;
    let actor = p.account()?;
    let app = node.app(&app_id).await?;
    if !node.sys(|s| fresh_record(s, &app).is_some_and(|app| s.manages_app(app, &actor))) {
        return Err(ApiError::forbidden(crate::system::ONLY_MANAGERS_TRANSFER));
    }
    // Before organizations are ordinary, an app in a personal organization handed to
    // someone outside it goes to theirs. After, an organization's app goes only to a
    // member of it: refused before the passkey is asked for (`TransferApp` refuses it too).
    let ordinary = node
        .require_features(&[crate::system::cluster_version::Feature::OrdinaryOrgs])
        .await
        .is_ok();
    if ordinary
        && !node.sys(|s| {
            s.orgs
                .get(&app.org)
                .and_then(|o| o.role(&body.to))
                .is_some_and(|r| r >= crate::types::OrgRole::Member)
        })
    {
        return Err(crate::node::rejected(crate::system::NEW_OWNER_NOT_A_MEMBER));
    }
    let consent = consent(
        &node,
        &p,
        &app_id,
        "__transfer",
        &json!({ "to": body.to }),
        body.confirm,
    )
    .await?;
    // Make the new owner's personal organization first if they have none.
    let leaves_personal = !ordinary
        && node.sys(|s| {
            s.orgs.get(&app.org).is_some_and(|o| {
                o.personal && o.role(&body.to).is_none() && s.accounts.contains_key(&body.to)
            })
        });
    if leaves_personal {
        let _theirs = personal_org(&node, &body.to).await?;
    }
    Ok(Json(
        node.sys_write(&SystemCmd::TransferApp {
            app_id,
            to: body.to,
            actor,
            consent,
            owner_in_org: true,
            owner_in_team: true,
        })
        .await?,
    ))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct MoveBody {
    to_org: String,
    #[serde(default)]
    bring_members: bool,
    #[serde(default)]
    confirm: Option<ConfirmBody>,
}

/// Move an app to another organization (`SystemCmd::MoveApp`), its data
/// where it is: whoever manages it where it is now, as an admin or the owner
/// of the target, with a passkey assertion over this move (412 first).
/// Answers where it went, its access there, whether an app team was made,
/// the accounts brought in and the invites revoked.
async fn app_move(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
    Json(body): Json<MoveBody>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    let p = holder(&node, &headers).await?;
    let actor = p.account()?;
    let app = node.app(&app_id).await?;
    if !node.sys(|s| fresh_record(s, &app).is_some_and(|app| s.manages_app(app, &actor))) {
        return Err(ApiError::forbidden(crate::system::ONLY_MANAGERS_MOVE));
    }
    if !node.sys(|s| s.is_org_admin(&body.to_org, &actor)) {
        return Err(ApiError::forbidden(crate::system::ONLY_ADMINS_MOVE_IN));
    }
    // Before the passkey step-up: a node that could not apply it refuses it anyway.
    node.require_features(&[
        Feature::MoveApp,
        Feature::MoveAccountsAppTeam,
        Feature::MoveOrgQuotas,
    ])
    .await?;
    let args = crate::system::move_app_args(&body.to_org, body.bring_members);
    let consent = consent(&node, &p, &app_id, "__move", &args, body.confirm).await?;
    Ok(Json(
        node.sys_write(&SystemCmd::MoveApp {
            app_id,
            to_org: body.to_org,
            actor,
            consent,
            bring_members: body.bring_members,
            nonce: b64url(&random::<16>()),
            all_apps_rows: true,
            accounts_in_app_team: true,
            org_quotas: true,
        })
        .await?,
    ))
}

async fn app_metadata(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
    Json(body): Json<MetadataBody>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    let p = signed_in(&node, &headers).await?;
    let actor = p.account()?;
    p.within(&node, &node.app(&app_id).await?)?;
    p.settings_write()?;
    let _ok = node
        .sys_write(&SystemCmd::SetAppMetadata {
            app_id,
            metadata: body.metadata,
            actor,
            owner_in_org: true,
            owner_in_team: true,
        })
        .await?;
    Ok(Json(json!({})))
}

/// A component app's xcalls (D25, D29): messages waiting in its outbox and its dead
/// letters, newest last. The app's owner's or an org admin's, from a replica.
async fn app_xcalls(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    let app = node.app(&app_id).await?;
    let p = gate(&node, &headers, &app, "*", false).await?;
    if !manages_app(&node, &app, p.caller.account()) {
        return Err(ApiError::forbidden(
            "only the app's owner or an org admin reads its xcalls",
        ));
    }
    if app.executor != crate::types::Executor::Component {
        return Err(ApiError::unsupported_engine());
    }
    Ok(Json(node.app_xcalls(&app).await?))
}

/// What the app's storage holds: keys, bytes, blobs, and when it last
/// committed a write (Unix seconds; `null` before its first).
async fn app_storage(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    let app = node.app(&app_id).await?;
    let _p = gate(&node, &headers, &app, "*", false).await?;
    let stats = node.storage_stats(&app).await?;
    let last_write = node.last_write_ms(&app).await;
    Ok(Json(json!({
        "keys": stats.keys,
        "bytes": stats.bytes,
        "blobs": stats.blobs.len(),
        "blobBytes": stats.blob_bytes,
        "lastWriteAt": last_write.map(|ms| ms / 1000),
    })))
}

/// The ABI the app's bundle embeds; `null` for none.
async fn app_abi(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    let app = node.app(&app_id).await?;
    let _p = gate(&node, &headers, &app, "*", false).await?;
    if app.executor != crate::types::Executor::Component {
        return Err(ApiError::unsupported_engine());
    }
    // The ABI the component embeds (its `calimero:abi` section), of the
    // bundle the app group runs.
    let bundle = node.running_bundle(&app);
    let wasm = node.bundle_wasm(&bundle).await?;
    let abi = crate::component::embedded_abi(&wasm);
    Ok(Json(json!({ "bundle": bundle, "abi": abi })))
}

/// The app's OpenAPI document, generated from the ABI of the bundle the app group runs
/// (`calimero_app_abi::openapi`), under the access `app_abi` has.
async fn app_openapi(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    let app = node.app(&app_id).await?;
    let _p = gate(&node, &headers, &app, "*", false).await?;
    if app.executor != crate::types::Executor::Component {
        return Err(ApiError::unsupported_engine());
    }
    let bundle = node.running_bundle(&app);
    let wasm = node.bundle_wasm(&bundle).await?;
    let Some(abi) = crate::component::embedded_abi(&wasm) else {
        return Err(ApiError::not_found("this app's bundle embeds no ABI"));
    };
    let abi = calimero_app_abi::AppAbi::from_json(&abi)
        .map_err(|e| ApiError::new(422, "BAD_ABI", &format!("the app's ABI: {e}")))?;
    let info = calimero_app_abi::openapi::Info {
        title: app.name.clone(),
        version: bundle,
        app: Some(app_id),
    };
    Ok(Json(calimero_app_abi::openapi::document(&abi, &info)))
}

/// One day of an app's usage book on the wire: every figure a decimal string (D20).
/// `wallMs` is for information only; the quota counts fuel (D76).
fn usage_day(day: u64, u: &crate::component::DayUsage) -> Value {
    json!({
        "date": crate::component::utc_date(day),
        "runs": u.runs.to_string(),
        "fuel": u.fuel.to_string(),
        "wallMs": u.wall_ms.to_string(),
        "writes": u.writes.to_string(),
        "bytesWritten": u.bytes_written.to_string(),
        "events": u.events.to_string(),
    })
}

/// What an app's runs used (Q20), for its owner and the admins of its organization
/// only: `today` and `days`, the last 30 days before it (oldest first), each UTC on
/// the node's app time; a day nothing ran is zeros, and so is every day of an app
/// recorded before components (SDK 0.12, no longer run).
async fn app_usage(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    let app = node.app(&app_id).await?;
    let p = gate(&node, &headers, &app, "*", false).await?;
    // After the gate: one who can't reach the app learns nothing of it (404).
    if !manages_app(&node, &app, p.caller.account()) {
        return Err(ApiError::forbidden(
            "only the app's owner or an org admin reads its usage",
        ));
    }
    let book = node.app_usage(&app).await?;
    let today = crate::component::utc_day(node.app_clock().now_ms());
    let day = |d: u64| usage_day(d, &book.on(d));
    let first = today.saturating_sub(crate::component::USAGE_DAYS - 1);
    Ok(Json(json!({
        "today": day(today),
        "days": (first..today).map(day).collect::<Vec<_>>(),
    })))
}

/// This area's routes, merged into [`super::router`].
/// The methods of `target` that the release `source` runs lists for its package in its
/// signed `uses` (`BundleUse::methods`): what a person is asked to let it call as them.
async fn delegable_methods(
    node: &Node,
    source: &crate::types::AppRecord,
    target: &crate::types::AppRecord,
) -> Result<Vec<String>, ApiError> {
    let (Some(from), Some(to)) = (node.release_of_app(source), node.release_of_app(target)) else {
        return Ok(Vec::new());
    };
    let card = crate::consent_page::release_capabilities(
        node,
        &from.at.package,
        &from.version,
        from.at.service.as_deref(),
    )
    .await?;
    let uses: BTreeMap<String, calimero_bundle::BundleUse> =
        serde_json::from_value(card["uses"].clone()).unwrap_or_default();
    let mut methods: Vec<String> = uses
        .into_values()
        .filter(|u| u.package == to.at.package)
        .flat_map(|u| u.methods)
        .collect();
    methods.sort();
    methods.dedup();
    Ok(methods)
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct DelegateBody {
    #[serde(default)]
    confirm: Option<ConfirmBody>,
}

/// What a grant of `source` over `target` would be for `account`: both apps, and the methods
/// of `target` that the release `source` runs lists for its package. `404` unless the account
/// reaches both and they are two apps of one organization; `409 NOTHING_TO_DELEGATE` when the
/// release lists none. The route and the consent page's `delegate` request share it.
pub(crate) async fn delegation_plan(
    node: &Node,
    account: &str,
    source_id: &str,
    target_id: &str,
) -> Result<
    (
        crate::types::AppRecord,
        crate::types::AppRecord,
        Vec<String>,
    ),
    ApiError,
> {
    let source = node.app(source_id).await?;
    let target = node.app(target_id).await?;
    let account = account.to_owned();
    let reaches =
        node.sys(|s| s.may_reach(&source, Some(&account)) && s.may_reach(&target, Some(&account)));
    if !reaches || source_id == target_id || source.org != target.org {
        return Err(ApiError::new(
            404,
            "NOT_FOUND",
            "no such pair of apps of one organization that you reach",
        ));
    }
    let methods = delegable_methods(node, &source, &target).await?;
    if methods.is_empty() {
        return Err(ApiError::new(
            409,
            "NOTHING_TO_DELEGATE",
            &format!("the release {source_id} runs lists no method of {target_id}'s package to call as you"),
        ));
    }
    Ok((source, target, methods))
}

/// Let `source`'s runs for the caller call `target` as them (`SystemCmd::SetDelegation`):
/// the methods the release `source` runs lists for `target`'s package in its `uses`, with
/// the caller's passkey over exactly those (412 first). The caller must reach both apps,
/// which are in one organization. From then on `target` sees the caller, not `source`, as
/// the caller of those methods when `source` calls them for this person, and reads
/// `env::via()` to know it came through `source`. `409 NOTHING_TO_DELEGATE` when the release
/// lists none. The account holder's own: an app's frontend asks for it on the consent page
/// (`delegate`), never here, as for any approval.
async fn app_delegate(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((source_id, target_id)): Path<(String, String)>,
    Json(body): Json<DelegateBody>,
) -> ApiResult {
    let p = holder(&node, &headers).await?;
    let source_id = node.canonical_app_id(&source_id);
    let target_id = node.canonical_app_id(&target_id);
    delegate(&node, &p, &source_id, &target_id, body.confirm).await
}

/// The grant itself, for the route and the consent page alike.
pub(crate) async fn delegate(
    node: &Node,
    p: &Principal,
    source_id: &str,
    target_id: &str,
    confirm: Option<ConfirmBody>,
) -> ApiResult {
    let account = p.account()?;
    let (_, _, methods) = delegation_plan(node, &account, source_id, target_id).await?;
    node.require_features(&[Feature::Delegation]).await?;
    let args = crate::system::delegation_args(target_id, &methods);
    let consent = consent(node, p, source_id, "__delegate", &args, confirm).await?;
    let _ok = node
        .sys_write(&SystemCmd::SetDelegation {
            account,
            source: source_id.to_owned(),
            target: target_id.to_owned(),
            methods: Some(methods.clone()),
            consent,
        })
        .await?;
    Ok(Json(
        json!({ "source": source_id, "target": target_id, "methods": methods }),
    ))
}

/// Take back what the caller let `source` do on `target` as them: no passkey, as it only
/// narrows. `404` when they granted nothing there.
async fn app_undelegate(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((source_id, target_id)): Path<(String, String)>,
) -> ApiResult {
    let source_id = node.canonical_app_id(&source_id);
    let target_id = node.canonical_app_id(&target_id);
    let p = signed_in(&node, &headers).await?;
    let account = p.account()?;
    let granted = node.sys(|s| {
        s.delegations
            .get(&account)
            .and_then(|g| g.get(&source_id))
            .is_some_and(|t| t.contains_key(&target_id))
    });
    if !granted {
        return Err(ApiError::new(
            404,
            "NOT_DELEGATED",
            &format!("you let {source_id} do nothing on {target_id} as you"),
        ));
    }
    node.require_features(&[Feature::Delegation]).await?;
    let _ok = node
        .sys_write(&SystemCmd::SetDelegation {
            account,
            source: source_id,
            target: target_id,
            methods: None,
            consent: Value::Null,
        })
        .await?;
    Ok(Json(json!({})))
}

/// What the caller lets `source` do as them: `{ source, delegations: { target: [method] } }`.
async fn app_delegations(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(source_id): Path<String>,
) -> ApiResult {
    let source_id = node.canonical_app_id(&source_id);
    let p = signed_in(&node, &headers).await?;
    let account = p.account()?;
    let source = node.app(&source_id).await?;
    p.within(&node, &source)?;
    let granted = node.sys(|s| {
        s.delegations
            .get(&account)
            .and_then(|g| g.get(&source_id))
            .cloned()
            .unwrap_or_default()
    });
    Ok(Json(json!({ "source": source_id, "delegations": granted })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SecretBody {
    value: String,
    #[serde(default)]
    confirm: Option<ConfirmBody>,
}

/// The app, its release's declared secrets, and the account, for whoever manages the app.
async fn secrets_gate(
    node: &Node,
    headers: &HeaderMap,
    app_id: &str,
    holder_only: bool,
) -> Result<
    (
        Principal,
        crate::types::AppRecord,
        BTreeMap<String, calimero_bundle::BundleSecret>,
    ),
    ApiError,
> {
    let app_id = node.canonical_app_id(app_id);
    let p = if holder_only {
        holder(node, headers).await?
    } else {
        signed_in(node, headers).await?
    };
    let app = node.app(&app_id).await?;
    p.within(node, &app)?;
    if !manages_app(node, &app, p.caller.account()) {
        return Err(ApiError::forbidden(
            "only the app's owner, or whoever manages its organization's apps, handles its secrets",
        ));
    }
    let declared = node
        .release_of_app(&app)
        .map(|r| r.version.secrets)
        .unwrap_or_default();
    Ok((p, app, declared))
}

/// The secrets the release an instance runs declares, and for each whether the instance
/// has its own value (`source: app`, with its fingerprint, who set it and when) or uses the
/// operator's (`source: cluster`). Never a value.
async fn app_secrets(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
) -> ApiResult {
    let (p, app, declared) = secrets_gate(&node, &headers, &app_id, false).await?;
    let own = node
        .view(&ViewRequest {
            app_id: app.app_id.clone(),
            method: crate::node::SECRETS_VIEW.to_owned(),
            args: Value::Null,
            caller: p.caller,
            strong: true,
            min_index: 0,
            via: None,
        })
        .await?
        .result;
    // The names its deployment holds a value of: the instance's owner learns only that
    // one is set there, never its fingerprint nor who set it.
    let package = node.release_of_app(&app).map(|r| r.at.package);
    let from_deployment: std::collections::BTreeSet<String> = node.sys(|s| {
        package
            .as_deref()
            .and_then(|p| s.deployment_secrets_of(&app.app_id, p))
            .map(|h| h.keys().cloned().collect())
            .unwrap_or_default()
    });
    let secrets: serde_json::Map<String, Value> = declared
        .into_iter()
        .map(|(name, d)| {
            let mut row = json!({
                "host": d.host, "paths": d.paths, "binding": d.describe(&name), "description": d.description,
                "required": d.required, "source": "cluster",
            });
            for (key, value) in [
                ("header", &d.header),
                ("format", &d.format),
                ("bodyField", &d.body_field),
            ] {
                if let Some(v) = value {
                    row[key] = json!(v);
                }
            }
            if let Some(o) = own.get(&name).filter(|o| o.is_object()) {
                row["source"] = json!("app");
                row["fingerprint"] = o["fingerprint"].clone();
                row["setBy"] = o["setBy"].clone();
                row["setAt"] = o["setAt"].clone();
            } else if from_deployment.contains(&name) {
                row["source"] = json!("deployment");
            }
            (name, row)
        })
        .collect();
    Ok(Json(json!({ "secrets": secrets })))
}

/// Set the instance's own value of a secret its release declares, attached only where
/// that secret's binding says and never shown again: the owner's (or whoever manages the
/// organization's apps), with a step-up over `{ name, fingerprint }` (never the value).
async fn app_secret_set(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((app_id, name)): Path<(String, String)>,
    Json(mut body): Json<SecretBody>,
) -> ApiResult {
    let (p, app, declared) = secrets_gate(&node, &headers, &app_id, true).await?;
    if !declared.contains_key(&name) {
        return Err(ApiError::not_found(
            "the release this app runs declares no such secret",
        ));
    }
    let value = crate::component::SecretValue(body.value);
    if !value.fits() {
        return Err(bad_secret_value());
    }
    // An older node would not apply the command: its replicas would hold no value.
    node.require_upgraded_nodes().await?;
    let fingerprint = value.fingerprint();
    let account = p.account()?;
    let _consent = consent(
        &node,
        &p,
        &app.app_id,
        "__set_secret",
        &json!({ "name": name, "fingerprint": fingerprint }),
        body.confirm.take(),
    )
    .await?;
    let a = node
        .call(&CallRequest {
            sys_index: p.sys_index,
            ..CallRequest::host(
                app.app_id.clone(),
                "__secret_set",
                json!({
                    "name": name, "value": value.0, "fingerprint": fingerprint,
                    "by": format!("account:{account}"), "at": now_secs(),
                }),
                p.caller,
            )
        })
        .await?;
    if a.status != Status::Ok {
        return Err(ApiError::conflict(a.error.as_str().unwrap_or("refused")));
    }
    Ok(Json(
        json!({ "name": name, "fingerprint": fingerprint, "index": a.index }),
    ))
}

/// Remove the instance's own value of a secret: the operator's applies again. Same rules
/// as setting one; a step-up over `{ name }`.
async fn app_secret_clear(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((app_id, name)): Path<(String, String)>,
    body: Option<Json<ConfirmOnlyBody>>,
) -> ApiResult {
    let (p, app, _declared) = secrets_gate(&node, &headers, &app_id, true).await?;
    node.require_upgraded_nodes().await?;
    let confirm = body.and_then(|Json(b)| b.confirm);
    let _consent = consent(
        &node,
        &p,
        &app.app_id,
        "__clear_secret",
        &json!({ "name": name }),
        confirm,
    )
    .await?;
    let a = node
        .call(&CallRequest {
            sys_index: p.sys_index,
            ..CallRequest::host(
                app.app_id.clone(),
                "__secret_clear",
                json!({ "name": name }),
                p.caller,
            )
        })
        .await?;
    if a.status != Status::Ok {
        return Err(ApiError::not_found(
            "this app holds no value of that secret",
        ));
    }
    Ok(Json(json!({ "name": name, "index": a.index })))
}

/// The references of the release `app` runs (`uses` in its signed manifest), by name; none
/// for an app without a release.
pub(crate) async fn release_uses(
    node: &Node,
    app: &crate::types::AppRecord,
) -> Result<BTreeMap<String, calimero_bundle::BundleUse>, ApiError> {
    let Some(release) = node.release_of_app(app) else {
        return Ok(BTreeMap::new());
    };
    let card = crate::consent_page::release_capabilities(
        node,
        &release.at.package,
        &release.version,
        release.at.service.as_deref(),
    )
    .await?;
    Ok(serde_json::from_value(card["uses"].clone()).unwrap_or_default())
}

/// Whether `release` is of `u`'s package and inside its range.
pub(crate) fn in_range(
    u: &calimero_bundle::BundleUse,
    release: &crate::system::PackageRef,
) -> bool {
    release.package == u.package
        && semver::VersionReq::parse(&u.range).is_ok_and(|range| {
            semver::Version::parse(&release.version).is_ok_and(|v| range.matches(&v))
        })
}

/// One link of an instance as `GET /apps/{app}/links` answers it: the reference (`null`
/// fields for a link its release no longer names), the app linked under it, the release
/// that app runs and whether it is in the reference's range (`null` with nothing linked).
fn link_row(
    node: &Node,
    name: &str,
    u: Option<&calimero_bundle::BundleUse>,
    linked: Option<&str>,
) -> Value {
    let release = linked
        .and_then(|id| node.sys(|s| s.apps.get(id).cloned()))
        .and_then(|a| node.release_of_app(&a))
        .map(|r| r.at);
    let fits = match (u, &release) {
        (Some(u), Some(r)) => Some(in_range(u, r)),
        _ => None,
    };
    json!({
        "name": name,
        "package": u.map(|u| &u.package),
        "range": u.map(|u| &u.range),
        "mode": u.map(|u| u.mode),
        "app": linked,
        "version": release.map(|r| r.version),
        "inRange": fits,
    })
}

/// An instance's links (`GET /apps/{app}/links`): each reference of the release it runs,
/// with the app linked under it (a unit member made with it, or an app its owner attached),
/// that app's release and whether it is in range. Who reaches the app reads them.
async fn app_links(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path(app_id): Path<String>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    let app = node.app(&app_id).await?;
    let _p = gate(&node, &headers, &app, "*", false).await?;
    let uses = release_uses(&node, &app).await?;
    let linked = |name: &str| {
        app.links
            .get(name)
            .or_else(|| app.attached.get(name))
            .map(String::as_str)
    };
    let mut rows: Vec<Value> = uses
        .iter()
        .map(|(name, u)| link_row(&node, name, Some(u), linked(name)))
        .collect();
    for name in app.links.keys().chain(app.attached.keys()) {
        if !uses.contains_key(name) {
            rows.push(link_row(&node, name, None, linked(name)));
        }
    }
    Ok(Json(json!({ "app": app_id, "links": rows })))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct LinkBody {
    app: String,
    #[serde(default)]
    confirm: Option<ConfirmBody>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct UnlinkBody {
    #[serde(default)]
    confirm: Option<ConfirmBody>,
}

/// Attach an app the caller runs to one of the instance's `attach` references
/// (`SystemCmd::LinkApp`): whoever manages both apps, in one organization, with a passkey
/// assertion (412 first). The app must run the reference's package; one outside its range is
/// attached, and the answer says so (`inRange: false`). Answers the link as `GET .../links`
/// lists it.
async fn app_link(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((app_id, name)): Path<(String, String)>,
    Json(body): Json<LinkBody>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    let target_id = node.canonical_app_id(&body.app);
    let p = holder(&node, &headers).await?;
    let actor = p.account()?;
    let app = node.app(&app_id).await?;
    let target = node.app(&target_id).await?;
    if !node.sys(|s| {
        fresh_record(s, &app).is_some_and(|app| s.manages_app(app, &actor))
            && fresh_record(s, &target).is_some_and(|target| s.manages_app(target, &actor))
    }) {
        return Err(ApiError::forbidden(crate::system::ONLY_MANAGERS_LINK));
    }
    if target_id == app_id || target.org != app.org {
        return Err(ApiError::bad_request(
            "NOT_ATTACHABLE",
            "only another app of the same organization is attached",
        ));
    }
    let uses = release_uses(&node, &app).await?;
    let Some(u) = uses
        .get(&name)
        .filter(|u| u.mode == calimero_bundle::UseMode::Attach)
    else {
        return Err(ApiError::new(
            409,
            "NOT_AN_ATTACH_REFERENCE",
            &format!("the release {app_id} runs names no `attach` reference {name}"),
        ));
    };
    let runs = node.release_of_app(&target).map(|r| r.at);
    if runs.as_ref().is_none_or(|r| r.package != u.package) {
        return Err(ApiError::new(
            409,
            "WRONG_PACKAGE",
            &format!(
                "{name} takes an app of {}: {target_id} runs another",
                u.package
            ),
        ));
    }
    // Before the passkey step-up: a node that could not apply it refuses it anyway.
    node.require_features(&[Feature::AttachApp]).await?;
    let args = crate::system::link_app_args(&name, Some(&target_id));
    let consent = consent(&node, &p, &app_id, "__link", &args, body.confirm).await?;
    let _ok = node
        .sys_write(&SystemCmd::LinkApp {
            app_id,
            name: name.clone(),
            target: Some(target_id.clone()),
            actor,
            consent,
        })
        .await?;
    Ok(Json(link_row(&node, &name, Some(u), Some(&target_id))))
}

/// Detach the app attached under `name` (`SystemCmd::LinkApp` with no target): whoever
/// manages the instance, with a passkey assertion (412 first). A unit member is never
/// detached. `404 NOT_ATTACHED` when nothing is attached there.
async fn app_unlink(
    State(node): State<Arc<Node>>,
    headers: HeaderMap,
    Path((app_id, name)): Path<(String, String)>,
    Json(body): Json<UnlinkBody>,
) -> ApiResult {
    let app_id = node.canonical_app_id(&app_id);
    let p = holder(&node, &headers).await?;
    let actor = p.account()?;
    let app = node.app(&app_id).await?;
    if !node.sys(|s| fresh_record(s, &app).is_some_and(|app| s.manages_app(app, &actor))) {
        return Err(ApiError::forbidden(crate::system::ONLY_MANAGERS_LINK));
    }
    if !app.attached.contains_key(&name) {
        return Err(ApiError::new(
            404,
            "NOT_ATTACHED",
            &format!("no app is attached to {app_id} as {name}"),
        ));
    }
    node.require_features(&[Feature::AttachApp]).await?;
    let args = crate::system::link_app_args(&name, None);
    let consent = consent(&node, &p, &app_id, "__link", &args, body.confirm).await?;
    let _ok = node
        .sys_write(&SystemCmd::LinkApp {
            app_id,
            name,
            target: None,
            actor,
            consent,
        })
        .await?;
    Ok(Json(json!({})))
}

pub(super) fn routes() -> Router<Arc<Node>> {
    Router::new()
        .route("/apps", post(app_create).get(app_list))
        .route("/apps/{app}", get(app_get).delete(app_delete))
        .route("/apps/{app}/metadata", post(app_metadata))
        .route("/apps/{app}/storage", get(app_storage))
        .route("/apps/{app}/usage", get(app_usage))
        .route("/apps/{app}/abi", get(app_abi))
        .route("/apps/{app}/openapi.json", get(app_openapi))
        .route("/apps/{app}/attestation", get(app_attestation))
        .route("/apps/{app}/access", post(app_access))
        .route(
            "/apps/{app}/invites",
            post(app_invite_create).get(app_invite_list),
        )
        .route("/apps/{app}/invites/{key}", delete(app_invite_revoke))
        .route("/apps/{app}/transfer", post(app_transfer))
        .route("/apps/{app}/move", post(app_move))
        .route("/apps/{app}/delegations", get(app_delegations))
        .route(
            "/apps/{app}/delegations/{target}",
            put(app_delegate).delete(app_undelegate),
        )
        .route("/apps/{app}/links", get(app_links))
        .route("/apps/{app}/links/{name}", put(app_link).delete(app_unlink))
        .route("/apps/{app}/xcalls", get(app_xcalls))
        .route("/apps/{app}/secrets", get(app_secrets))
        .route(
            "/apps/{app}/secrets/{name}",
            put(app_secret_set).delete(app_secret_clear),
        )
}
