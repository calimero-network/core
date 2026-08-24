use actix::{ActorResponse, Handler, Message};
use calimero_context_client::group::{
    GroupDeviceEntry, ListGroupDevicesRequest, ListGroupDevicesResponse,
};
use calimero_governance_store::{
    AccountBindingRepository, DeviceBinding, MembershipRepository, MetaRepository,
    NamespaceRepository,
};
use calimero_primitives::identity::MemberPrincipal;
use eyre::bail;

use crate::ContextManager;
use calimero_governance_store;

/// Does `binding` belong to the principal a caller asked about?
///
/// The two spellings ask genuinely different questions of the same row — "which
/// devices does this person have" versus "whose device is this key" — and both
/// are answered by matching one field, so neither needs its own lookup.
///
/// A key match is not narrowed to one row on purpose. Nothing constrains two
/// devices to distinct signing keys, and a caller resolving a key needs to see a
/// collision if there is one rather than be handed whichever row a point lookup
/// reached first.
fn matches(binding: &DeviceBinding, member: &MemberPrincipal) -> bool {
    match *member {
        MemberPrincipal::Account(account) => binding.account == account,
        MemberPrincipal::Key(key) => binding.sign_pk == key,
    }
}

impl Handler<ListGroupDevicesRequest> for ContextManager {
    type Result = ActorResponse<Self, <ListGroupDevicesRequest as Message>::Result>;

    fn handle(
        &mut self,
        ListGroupDevicesRequest {
            group_id,
            member,
            offset,
            limit,
        }: ListGroupDevicesRequest,
        _ctx: &mut Self::Context,
    ) -> Self::Result {
        let result = (|| -> eyre::Result<ListGroupDevicesResponse> {
            if MetaRepository::new(&self.datastore)
                .load(&group_id)?
                .is_none()
            {
                bail!("group '{group_id:?}' not found");
            }

            // Same gate as the member listing, for the same reason: these rows
            // say who may act in this namespace, so they are readable by a node
            // that belongs to it and not by one that merely knows its id.
            let Some((node_identity, _)) = self.node_signing_key(&group_id) else {
                bail!("node has no group identity configured");
            };
            let node_account =
                crate::member_account::require(&self.datastore, &group_id, &node_identity)?;
            if MembershipRepository::new(&self.datastore).check_path(&group_id, &node_account)?
                == calimero_governance_store::MembershipPath::None
            {
                // Typed so the admin API surfaces this precondition as a 403,
                // not a generic 500 (see `parse_api_error`).
                return Err(crate::error::ContextError::NotAGroupMember {
                    group_id: format!("{group_id:?}"),
                }
                .into());
            }

            // Bindings live at the NAMESPACE: they are written where the
            // credential arrived, which is the namespace a member joined, and a
            // subgroup holds none of its own. A caller asking a subgroup means
            // "the devices behind this subgroup's members", so resolving up is
            // the answer rather than an empty list.
            let namespace = NamespaceRepository::new(&self.datastore).resolve(&group_id)?;

            let mut bindings =
                AccountBindingRepository::new(&self.datastore).live_bindings(&namespace)?;
            if let Some(ref member) = member {
                bindings.retain(|binding| matches(binding, member));
            }
            // Stable, path-independent pagination: store order is not a
            // contract, so two pages of one listing could otherwise skip or
            // repeat a device. Device id is unique per row, which account and
            // signing key are not.
            bindings.sort_by_key(|binding| binding.device);

            let devices = bindings
                .into_iter()
                .skip(offset)
                .take(limit)
                .map(|binding| GroupDeviceEntry {
                    device: binding.device,
                    account: binding.account,
                    signing_key: binding.sign_pk,
                    device_epoch: binding.device_epoch,
                })
                .collect();

            Ok(ListGroupDevicesResponse { devices })
        })();

        ActorResponse::reply(result)
    }
}

#[cfg(test)]
mod tests {
    use calimero_account::{AccountId, DeviceId, KemPublicKey};
    use calimero_primitives::identity::{PrivateKey, PublicKey};

    use super::*;

    fn key(seed: u8) -> PublicKey {
        PrivateKey::from([seed; 32]).public_key()
    }

    fn binding(account: u8, device: u8, sign: u8) -> DeviceBinding {
        DeviceBinding {
            device: DeviceId::from([device; 32]),
            account: AccountId::from([account; 32]),
            sign_pk: key(sign),
            kem_pk: *KemPublicKey::from([0x2B; 32]).as_bytes(),
            device_epoch: 0,
        }
    }

    #[test]
    fn an_account_selects_every_device_that_speaks_for_it() {
        let mine = binding(0xA1, 0x01, 0x11);
        let also_mine = binding(0xA1, 0x02, 0x12);
        let theirs = binding(0xB2, 0x03, 0x13);
        let member = MemberPrincipal::Account(AccountId::from([0xA1; 32]));

        assert!(matches(&mine, &member));
        assert!(matches(&also_mine, &member));
        assert!(!matches(&theirs, &member));
    }

    /// The resolution the `add_group_members` key form was standing in for: a
    /// caller holds a key, and the row it selects names the account.
    #[test]
    fn a_key_selects_the_device_that_presents_it() {
        let mine = binding(0xA1, 0x01, 0x11);
        let sibling_device = binding(0xA1, 0x02, 0x12);

        assert!(matches(&mine, &MemberPrincipal::Key(key(0x11))));
        assert!(
            !matches(&sibling_device, &MemberPrincipal::Key(key(0x11))),
            "a key names one device, not the person's whole set"
        );
    }

    /// An account is not a signing key and must not select as one, even though
    /// both are 32 bytes. The encodings make this unreachable through the API;
    /// the match arms make it unreachable here too.
    #[test]
    fn a_principal_never_matches_the_other_kinds_field() {
        let row = binding(0xA1, 0x01, 0x11);

        assert!(!matches(
            &row,
            &MemberPrincipal::Key(PublicKey::from([0xA1; 32]))
        ));
        assert!(!matches(
            &row,
            &MemberPrincipal::Account(AccountId::from(*key(0x11)))
        ));
    }
}
