use calimero_context::group_store::{
    GroupKeyring, MetadataRepository, NamespaceRepository, SigningKeysRepository,
};
use std::sync::Arc;

use axum::extract::Path;
use axum::response::IntoResponse;
use axum::Extension;
use axum::Json;
use calimero_context::governance_broadcast::ObserveDelivery;
use serde::{Deserialize, Serialize};
use tracing::{error, info, warn};

use crate::admin::handlers::groups::parse_group_id;
use crate::admin::service::{parse_api_error, ApiResponse};
use crate::auth::AuthenticatedKey;
use crate::AdminState;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateGroupInNamespaceBody {
    pub group_name: Option<String>,
    /// Optional subgroup visibility at birth (#2771): `"open"` or
    /// `"restricted"`. Absent ⇒ `"restricted"` (preserves legacy behavior).
    /// A born-Open subgroup is Open at `SubgroupCreated`-event time, so
    /// `tee_subgroup_admit` skips it (TEE reads via inheritance) and no
    /// transient direct `ReadOnlyTee` row is created.
    pub visibility: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateGroupInNamespaceResponseData {
    pub group_id: String,
}

#[derive(Debug, Serialize)]
pub struct CreateGroupInNamespaceResponse {
    pub data: CreateGroupInNamespaceResponseData,
}

pub async fn handler(
    Path(namespace_id_hex): Path<String>,
    Extension(state): Extension<Arc<AdminState>>,
    auth_key: Option<Extension<AuthenticatedKey>>,
    Json(body): Json<CreateGroupInNamespaceBody>,
) -> impl IntoResponse {
    let namespace_id = match parse_group_id(&namespace_id_hex) {
        Ok(id) => id,
        Err(err) => return err.into_response(),
    };
    let requester = auth_key.map(|Extension(k)| k.0);

    info!(
        namespace_id = %namespace_id_hex,
        "Creating group in namespace via namespace governance"
    );

    let group_id: [u8; 32] = {
        use rand::Rng;
        rand::thread_rng().gen()
    };

    match NamespaceRepository::new(&state.store).parent(&namespace_id) {
        Ok(Some(_)) => {
            return parse_api_error(eyre::eyre!("namespace_id must reference a root group"))
                .into_response();
        }
        Ok(None) => {}
        Err(err) => return parse_api_error(err).into_response(),
    }

    let (resolved_ns_id, signer_pk, sk_bytes, _sender) =
        match NamespaceRepository::new(&state.store).get_or_create_identity(&namespace_id) {
            Ok(r) => r,
            Err(err) => {
                error!(?err, "Failed to resolve namespace identity");
                return parse_api_error(err).into_response();
            }
        };

    if let Some(requester) = requester {
        if requester != signer_pk {
            return parse_api_error(eyre::eyre!(
                "requester does not match local namespace identity"
            ))
            .into_response();
        }
    }

    let signer_sk = calimero_primitives::identity::PrivateKey::from(sk_bytes);

    // Map the optional `visibility` field to the op's `restricted` flag.
    // Default (absent / unrecognized) ⇒ Restricted, matching legacy behavior.
    let restricted = match body.visibility.as_deref() {
        Some(v) if v.eq_ignore_ascii_case("open") => false,
        Some(v) if v.eq_ignore_ascii_case("restricted") => true,
        Some(other) => {
            return parse_api_error(eyre::eyre!(
                "invalid visibility '{other}': expected \"open\" or \"restricted\""
            ))
            .into_response();
        }
        None => true,
    };

    let group_id_cgid = calimero_context_config::types::ContextGroupId::from(group_id);

    // Mint the subgroup's signing key AND group key BEFORE applying the op.
    //
    // The apply path drains `OpEvent::SubgroupCreated` *after* it persists the
    // root op (emit-after-persist, #2770). The `tee_subgroup_admit` subscriber
    // reacts to that event by reading this subgroup's group key
    // (`GroupKeyring::load_current_key`) to decide whether it is the key-holder
    // that should admit the namespace's root TEE members. If the key were minted
    // only after the apply returns (as it used to be), the subscriber would race
    // ahead of the write, observe `None`, and wrongly skip — leaving the TEE out
    // of every Restricted subgroup created via this REST path. Writing both keys
    // first mirrors the actor handler (crates/context/src/handlers/create_group.rs)
    // and makes the key visible the instant the event fires. It also means a
    // failed publish no longer silently skips key creation.
    if let Err(err) =
        SigningKeysRepository::new(&state.store).store_key(&group_id_cgid, &signer_pk, &sk_bytes)
    {
        error!(
            group_id=%hex::encode(group_id_cgid.to_bytes()),
            ?err,
            "Failed to store admin signing key before group create"
        );
        return parse_api_error(eyre::eyre!("failed to store subgroup signing key"))
            .into_response();
    }
    {
        let group_key: [u8; 32] = {
            use rand::Rng;
            rand::thread_rng().gen()
        };
        if let Err(err) = GroupKeyring::new(&state.store, group_id_cgid).store_key(&group_key) {
            error!(
                group_id=%hex::encode(group_id_cgid.to_bytes()),
                ?err,
                "Failed to generate subgroup group key before group create"
            );
            return parse_api_error(eyre::eyre!("failed to mint subgroup group key"))
                .into_response();
        }
    }

    // Strict-tree refactor: GroupCreated atomically nests the new group under
    // the namespace root in one op. Previous two-op pattern (GroupCreated then
    // GroupNested) is collapsed — orphan state is no longer reachable. See
    // docs/superpowers/specs/2026-04-22-strict-group-tree-and-cascade-delete.md
    let op = calimero_context_client::local_governance::NamespaceOp::Root(
        calimero_context_client::local_governance::RootOp::GroupCreated {
            group_id,
            parent_id: namespace_id.to_bytes(),
            restricted,
        },
    );

    match calimero_context::group_store::sign_apply_and_publish_namespace_op(
        &state.store,
        &state.node_client,
        state.ctx_client.ack_router(),
        resolved_ns_id.to_bytes().into(),
        &signer_sk,
        op,
    )
    .await
    {
        Ok(report) => {
            report.observe("create_group_in_namespace", "GroupCreated");
            let group_id = group_id_cgid;

            if let Some(name) = body.group_name.as_deref() {
                // Seed the subgroup's initial metadata record stamped with the
                // creator's identity / wall-clock — not the zero-value
                // `Default` (which would surface as misleading provenance via
                // the API); later `GroupOp::GroupMetadataSet` ops supersede it.
                // Validate the name here too — this seed bypasses the op-apply
                // validator.
                if let Err(reason) = calimero_primitives::metadata::validate_metadata_payload(
                    Some(name),
                    &std::collections::BTreeMap::new(),
                ) {
                    warn!(
                        group_id=%hex::encode(group_id.to_bytes()),
                        %reason,
                        "Group created but the requested name is invalid; not persisted"
                    );
                } else if let Err(err) = MetadataRepository::new(&state.store).set_group(
                    &group_id,
                    &calimero_primitives::metadata::MetadataRecord {
                        name: body.group_name.clone(),
                        data: std::collections::BTreeMap::new(),
                        updated_at: calimero_context::group_store::now_millis(),
                        updated_by: signer_pk,
                    },
                ) {
                    warn!(
                        group_id=%hex::encode(group_id.to_bytes()),
                        ?err,
                        "Group created but failed to persist name"
                    );
                }
            }

            ApiResponse {
                payload: CreateGroupInNamespaceResponse {
                    data: CreateGroupInNamespaceResponseData {
                        group_id: hex::encode(group_id.to_bytes()),
                    },
                },
            }
            .into_response()
        }
        Err(err) => {
            error!(?err, "Failed to create group in namespace");
            parse_api_error(err).into_response()
        }
    }
}
