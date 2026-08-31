use calimero_server_primitives::admin::{
    AccountApplicationsApiResponse, AccountDevicesApiResponse, AccountPairCompleteApiRequest,
    AccountPairInitApiRequest, AdmitJoinApiRequest, AdmitJoinApiResponse,
    CreateGroupInvitationApiRequest, CreateNamespaceApiRequest, CreateNamespaceApiResponse,
    DeleteNamespaceApiRequest, DeleteNamespaceApiResponse, GetNamespaceApiResponse,
    JoinGroupApiRequest, JoinNamespaceApiResponse, ListNamespaceGroupsApiResponse,
    ListNamespacesApiResponse, NamespaceApiResponse, NodeIdentityApiResponse,
    PairDeviceCompleteApiResponse, PairDeviceInitApiResponse, RelinkDeviceApiRequest,
    RelinkDeviceApiResponse, RevokeDeviceApiRequest, RevokeDeviceApiResponse,
};
use eyre::Result;
use serde::Serialize;

use super::{ClientAuthenticator, ClientStorage};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateGroupInNamespaceApiRequest {
    group_name: Option<String>,
}

impl<A, S> super::Client<A, S>
where
    A: ClientAuthenticator + Clone + Send + Sync,
    S: ClientStorage + Clone + Send + Sync,
{
    pub async fn list_namespaces(&self) -> Result<ListNamespacesApiResponse> {
        let response = self.connection.get("admin-api/namespaces").await?;
        Ok(response)
    }

    pub async fn list_namespaces_for_application(
        &self,
        application_id: &str,
    ) -> Result<ListNamespacesApiResponse> {
        let response = self
            .connection
            .get(&format!(
                "admin-api/namespaces/for-application/{application_id}"
            ))
            .await?;
        Ok(response)
    }

    pub async fn create_namespace(
        &self,
        request: CreateNamespaceApiRequest,
    ) -> Result<CreateNamespaceApiResponse> {
        let response = self
            .connection
            .post("admin-api/namespaces", request)
            .await?;
        Ok(response)
    }

    pub async fn get_namespace(&self, namespace_id: &str) -> Result<NamespaceApiResponse> {
        let response = self
            .connection
            .get(&format!("admin-api/namespaces/{namespace_id}"))
            .await?;
        let response: GetNamespaceApiResponse = response;
        Ok(response.data)
    }

    pub async fn delete_namespace(
        &self,
        namespace_id: &str,
        request: DeleteNamespaceApiRequest,
    ) -> Result<DeleteNamespaceApiResponse> {
        let response = self
            .connection
            .delete_with_body(&format!("admin-api/namespaces/{namespace_id}"), request)
            .await?;
        Ok(response)
    }

    /// Who this node is: the account it writes as, the device it is, and the key
    /// it signs with.
    ///
    /// Takes no namespace. Each field is node-level — one root key is one
    /// account everywhere, a node is one device, and it signs with one key — so
    /// the namespaced endpoint this replaces returned the same answer whatever
    /// it was given, which read as though the answer varied by scope.
    ///
    /// Read-only and answerable before any device is enrolled: the account is
    /// derived from this node's root. Everything that names an account —
    /// granting a writer, revoking a device — starts here.
    pub async fn get_node_identity(&self) -> Result<NodeIdentityApiResponse> {
        let response = self.connection.get("admin-api/identity").await?;
        Ok(response)
    }

    /// Mint a device on this node for an account that already exists elsewhere
    /// - the first half of pairing.
    ///
    /// Takes the SET of namespaces the device will listen on, because a node
    /// that is a member of nothing can neither read its account's namespaces off
    /// a DAG nor derive them. Needs no scope key: nothing is published here. It
    /// returns the device id and agreement key that the account holder certifies
    /// in the second half.
    pub async fn pair_device_init(
        &self,
        request: AccountPairInitApiRequest,
    ) -> Result<PairDeviceInitApiResponse> {
        let response = self
            .connection
            .post("admin-api/account/pair-init", request)
            .await?;
        Ok(response)
    }

    /// Certify a device another node minted, link it, and deliver the scope
    /// keys - the second half of pairing.
    ///
    /// Run on the node that holds the account, and scoped by APPLICATION rather
    /// than by namespace: naming none means every namespace this node takes part
    /// in. Needs the current scope key there: the link is an encrypted group op,
    /// and the delivery is that same key wrapped for the new device.
    pub async fn pair_device_complete(
        &self,
        request: AccountPairCompleteApiRequest,
    ) -> Result<PairDeviceCompleteApiResponse> {
        let response = self
            .connection
            .post("admin-api/account/pair-complete", request)
            .await?;
        Ok(response)
    }

    /// Repair or widen the reach of a device this account already certified.
    ///
    /// Re-runs pairing's fan-out against the namespaces this node takes part in
    /// now, which is what closes the drift a namespace gained afterwards leaves
    /// behind. Naming no application repairs without widening.
    pub async fn relink_device(
        &self,
        device_id: &str,
        request: RelinkDeviceApiRequest,
    ) -> Result<RelinkDeviceApiResponse> {
        let response = self
            .connection
            .post(
                &format!("admin-api/account/devices/{device_id}/relink"),
                request,
            )
            .await?;
        Ok(response)
    }

    /// Every device of this account, with the scope and bindings this node can see.
    ///
    /// Joined from the node-local certificate cache and the live bindings of every
    /// namespace this node takes part in, so it reports devices this node never
    /// certified as well as the ones it did.
    pub async fn list_account_devices(&self) -> Result<AccountDevicesApiResponse> {
        let response = self.connection.get("admin-api/account/devices").await?;
        Ok(response)
    }

    /// The applications this account speaks in.
    ///
    /// The only route by which a device that is a member of nothing can learn
    /// them: a namespace summary is withheld from non-members.
    pub async fn list_account_applications(&self) -> Result<AccountApplicationsApiResponse> {
        let response = self
            .connection
            .get("admin-api/account/applications")
            .await?;
        Ok(response)
    }

    /// Withdraw a device from an account, terminally.
    ///
    /// An admin may revoke any device and rotates the scope key in the same op.
    /// The account holder may revoke its own with a root-signed proof, but
    /// cannot rotate — so the device stops writing at once and keeps reading
    /// until an admin rotates.
    pub async fn revoke_device(
        &self,
        namespace_id: &str,
        request: RevokeDeviceApiRequest,
    ) -> Result<RevokeDeviceApiResponse> {
        let response = self
            .connection
            .post(
                &format!("admin-api/namespaces/{namespace_id}/account/revoke"),
                request,
            )
            .await?;
        Ok(response)
    }

    pub async fn create_namespace_invitation(
        &self,
        namespace_id: &str,
        request: CreateGroupInvitationApiRequest,
    ) -> Result<serde_json::Value> {
        let response = self
            .connection
            .post(
                &format!("admin-api/namespaces/{namespace_id}/invite"),
                request,
            )
            .await?;
        Ok(response)
    }

    pub async fn join_namespace(
        &self,
        namespace_id: &str,
        request: JoinGroupApiRequest,
    ) -> Result<JoinNamespaceApiResponse> {
        let response = self
            .connection
            .post(
                &format!("admin-api/namespaces/{namespace_id}/join"),
                request,
            )
            .await?;
        Ok(response)
    }

    /// Hand a join this caller already signed to a node the inviter named as an
    /// admitter, for that node to publish.
    ///
    /// The counterpart to [`Self::join_namespace`], which publishes from the node
    /// serving the call. A keyholder has no such node — an account, a device
    /// certificate signed offline, and nowhere to publish from — so it signs the
    /// membership op itself and presents it here.
    ///
    /// Handing the op to somebody else is safe because it is signed by the
    /// joiner's device key, and every peer checks that key against the credential
    /// the op carries when applying a join. The admitter carries the claim and
    /// cannot author one: it may refuse to publish, and nothing else.
    pub async fn admit_join(
        &self,
        namespace_id: &str,
        request: AdmitJoinApiRequest,
    ) -> Result<AdmitJoinApiResponse> {
        let response = self
            .connection
            .post(
                &format!("admin-api/namespaces/{namespace_id}/admit"),
                request,
            )
            .await?;
        Ok(response)
    }

    pub async fn list_namespace_groups(
        &self,
        namespace_id: &str,
    ) -> Result<ListNamespaceGroupsApiResponse> {
        let response = self
            .connection
            .get(&format!("admin-api/namespaces/{namespace_id}/groups"))
            .await?;
        Ok(response)
    }

    pub async fn create_group_in_namespace(
        &self,
        namespace_id: &str,
        group_name: Option<String>,
    ) -> Result<serde_json::Value> {
        let response = self
            .connection
            .post(
                &format!("admin-api/namespaces/{namespace_id}/groups"),
                CreateGroupInNamespaceApiRequest { group_name },
            )
            .await?;
        Ok(response)
    }
}
