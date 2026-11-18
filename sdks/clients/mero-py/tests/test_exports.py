"""The package exports the Calimero 1.0 client and nothing of v1's.

The v1 client (`create_connection`, `create_client`, v1's `Client`, warrants,
request proofs, `TeePolicy`, the token cache) was deleted with v1's peer-to-peer
node; CHANGELOG.md says what replaces each name. This keeps one from coming back
half-wired. `Client` is a name again, for the 1.0 client: the same class as
`CloudClient`.
"""

import inspect

import calimero
import calimero_client_py

CLIENT = {
    "Client",
    "CloudClient",
    "CloudError",
    "FeatureNotActive",
    "LoginRequired",
    "SoftPasskey",
    "WrongNode",
}

V1 = {
    "create_connection",
    "create_client",
    "ConnectionInfo",
    "JwtToken",
    "ClientError",
    "AuthMode",
    "TeePolicy",
    "get_token_cache_path",
    "get_token_cache_dir",
    "save_token_cache",
    "sign_warrant",
    "sign_request",
}


def test_the_extension_exports_the_client():
    for name in CLIENT:
        assert hasattr(calimero_client_py, name), name
    assert isinstance(calimero_client_py.VERSION, str)


def test_the_package_reexports_exactly_the_client():
    assert set(calimero.__all__) == CLIENT
    for name in CLIENT:
        assert getattr(calimero, name) is getattr(calimero_client_py, name)


def test_no_v1_name_is_left():
    for module in (calimero, calimero_client_py):
        left = sorted(n for n in V1 if hasattr(module, n))
        assert not left, f"{module.__name__} still exports {left}"


def test_cloud_error_is_an_exception():
    assert issubclass(calimero_client_py.CloudError, Exception)


def test_login_required_and_wrong_node_are_cloud_errors():
    # Code that catches `CloudError` keeps catching both.
    for name in ("LoginRequired", "WrongNode", "FeatureNotActive"):
        assert issubclass(
            getattr(calimero_client_py, name), calimero_client_py.CloudError
        )


def test_the_client_names_the_node_in_use():
    # A property: `base_url` until a `421 WRONG_NODE` moved the client on.
    assert hasattr(calimero_client_py.Client, "node_url")


def test_client_is_the_cloud_client():
    assert calimero_client_py.Client is calimero_client_py.CloudClient
    assert calimero.Client is calimero.CloudClient


# The Ops API calls mdma makes through `CloudClient` (mdma_common.cloud_v2), and
# the rest of core's `calimero-client`: a missing one is a missing binding.
METHODS = {
    # a cluster by name, from the node directory
    "connect",
    # session and account
    "sign_up",
    "sign_in",
    "refresh",
    "use_session",
    "use_api_key",
    "session",
    "add_passkey",
    "normalize_recovery_code",
    "recovery_code_key",
    "start_account_recovery",
    "finish_account_recovery",
    "api_key_public",
    # packages
    # apps
    "verify_app_cluster",
    "events",
    "presence_stream",
    # organizations and teams
    "new_recovery_seed",
    "recovery_public_key",
    "sign_recovery",
    "start_recovery",
    # Ops API
    "use_operator_key",
    "operator_public_key",
    # any path
    "get",
    "get_text",
    "post",
    "put",
    "delete",
    "post_confirmed",
    "delete_confirmed",
    "post_operator_signed",
    "delete_operator_signed",
    "attestation",
}

# The routes the methods above no longer cover: one facade method each
# (`client.<group>.<method>`, generated from core's OpenAPI document).
FACADE = {
    "apps.abi",
    "apps.access",
    "apps.blob_delete",
    "apps.blob_download",
    "apps.blob_info",
    "apps.blob_list",
    "apps.blob_upload",
    "apps.call",
    "apps.create",
    "apps.delete",
    "apps.get",
    "apps.list",
    "apps.metadata",
    "apps.move",
    "apps.presence_get",
    "apps.presence_publish",
    "apps.storage",
    "apps.transfer",
    "apps.upgrade",
    "apps.view",
    "auth.account_recovery_cancel",
    "auth.activity_list",
    "auth.alias_delete",
    "auth.alias_get",
    "auth.alias_list",
    "auth.alias_set",
    "auth.apikey_create",
    "auth.apikey_list",
    "auth.apikey_revoke",
    "auth.logout",
    "auth.logout_all",
    "auth.me",
    "auth.passkey_label",
    "auth.passkey_remove",
    "auth.recovery_create",
    "auth.recovery_remove",
    "auth.session_revoke",
    "auth.sessions_list",
    "auth.usage",
    "ops.backup",
    "ops.metrics",
    "ops.org_upgrade",
    "ops.outbound",
    "ops.package_install",
    "ops.package_list",
    "ops.package_uninstall",
    "ops.package_upload",
    "ops.package_version_uninstall",
    "ops.quotas_get",
    "ops.quotas_set",
    "ops.register_node",
    "ops.release",
    "ops.remove_node",
    "ops.served",
    "ops.set_replicas",
    "ops.status",
    "ops.upgrade",
    "orgs.app_team_list",
    "orgs.app_teams_scope",
    "orgs.capabilities",
    "orgs.create",
    "orgs.delete",
    "orgs.get",
    "orgs.invite_create",
    "orgs.invite_redeem",
    "orgs.member_metadata",
    "orgs.member_remove_teams",
    "orgs.member_set_teams",
    "orgs.members",
    "orgs.metadata",
    "orgs.outbound_approval",
    "orgs.recovery_cancel",
    "orgs.recovery_finish",
    "orgs.recovery_set",
    "orgs.release_approval",
    "orgs.remove_member",
    "orgs.set_member",
    "orgs.team_create",
    "orgs.team_delete",
    "orgs.team_get",
    "orgs.team_remove_member",
    "orgs.team_set_member",
    "orgs.team_update",
    "orgs.upgrade",
    "orgs.upgrade_retry",
    "orgs.upgrade_status",
    "packages.bundle_put",
    "packages.get",
    "packages.install",
    "packages.list",
    "packages.origins_add",
    "packages.origins_list",
    "packages.origins_remove",
    "packages.uninstall",
    "packages.upload",
    "packages.version_uninstall",
}


def test_the_client_binds_every_method():
    missing = sorted(
        m for m in METHODS if not callable(getattr(calimero_client_py.Client, m, None))
    )
    assert not missing, f"Client lacks {missing}"


def test_the_facade_binds_every_route():
    from calimero_client_py import facade

    missing = sorted(
        f
        for f in FACADE
        if not callable(getattr(facade.GROUPS[f.split(".")[0]], f.split(".")[1], None))
    )
    assert not missing, f"the facade lacks {missing}"


def test_team_delete_takes_an_optional_passkey():
    # A team holding apps is deleted with the deleter's passkey (core's `412
    # CONFIRM_REQUIRED` naming them); an empty one without.
    from calimero_client_py.facade import Orgs

    params = inspect.signature(Orgs.team_delete).parameters
    assert list(params)[1:] == ["org", "team", "confirm"]
    assert params["confirm"].default is None
