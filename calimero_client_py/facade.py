"""The operations of core's node API, one method each: generated, do not edit.

Made by scripts/generate_facade.py from openapi/openapi.json and the
generated calls in `calimero_client_py.openapi`. Reach them on a client:
`client.orgs.invite_create(org=..., role="member")`. Arguments are
keyword-only: the path's, the query's and the JSON body's fields; a
raw-byte body is `body=`. Each returns the generated model of the success
answer and raises `CloudError` on a refusal. A `412 CONFIRM_REQUIRED` is
answered with the client's passkey (`Client.use_passkey`) and a `428
OPERATOR_SIGNATURE_REQUIRED` with its operator key
or signer (`Client.use_operator_key`, `Client.use_operator_signer`); per
call, `confirm=` takes a passkey, a ready `confirm` value or `False`, and
`operator_proof=` an operator seed (hex), a signer, a ready
`operatorProof` value or `False`. `client.json.<group>` answers JSON.
"""

from .openapi.api.accounts import account_apps as _account_apps
from .openapi.api.accounts import account_backup_key as _account_backup_key
from .openapi.api.accounts import account_proof_prove as _account_proof_prove
from .openapi.api.accounts import account_recovery_begin as _account_recovery_begin
from .openapi.api.accounts import account_recovery_cancel as _account_recovery_cancel
from .openapi.api.accounts import account_recovery_finish as _account_recovery_finish
from .openapi.api.accounts import account_recovery_start as _account_recovery_start
from .openapi.api.accounts import account_set_backup_key as _account_set_backup_key
from .openapi.api.accounts import activity_list as _activity_list
from .openapi.api.accounts import alias_delete as _alias_delete
from .openapi.api.accounts import alias_get as _alias_get
from .openapi.api.accounts import alias_list as _alias_list
from .openapi.api.accounts import alias_set as _alias_set
from .openapi.api.accounts import apikey_create as _apikey_create
from .openapi.api.accounts import apikey_list as _apikey_list
from .openapi.api.accounts import apikey_revoke as _apikey_revoke
from .openapi.api.accounts import login_begin as _login_begin
from .openapi.api.accounts import login_finish as _login_finish
from .openapi.api.accounts import logout as _logout
from .openapi.api.accounts import logout_all as _logout_all
from .openapi.api.accounts import me as _me
from .openapi.api.accounts import passkey_add_begin as _passkey_add_begin
from .openapi.api.accounts import passkey_add_finish as _passkey_add_finish
from .openapi.api.accounts import passkey_label as _passkey_label
from .openapi.api.accounts import passkey_remove as _passkey_remove
from .openapi.api.accounts import recovery_create as _recovery_create
from .openapi.api.accounts import recovery_remove as _recovery_remove
from .openapi.api.accounts import refresh as _refresh
from .openapi.api.accounts import register_begin as _register_begin
from .openapi.api.accounts import register_finish as _register_finish
from .openapi.api.accounts import session_revoke as _session_revoke
from .openapi.api.accounts import sessions_list as _sessions_list
from .openapi.api.accounts import usage as _usage
from .openapi.api.apps import app_abi as _app_abi
from .openapi.api.apps import app_access as _app_access
from .openapi.api.apps import app_attestation as _app_attestation
from .openapi.api.apps import app_create as _app_create
from .openapi.api.apps import app_delete as _app_delete
from .openapi.api.apps import app_get as _app_get
from .openapi.api.apps import app_list as _app_list
from .openapi.api.apps import app_metadata as _app_metadata
from .openapi.api.apps import app_method as _app_method
from .openapi.api.apps import app_move as _app_move
from .openapi.api.apps import app_openapi as _app_openapi
from .openapi.api.apps import app_run as _app_run
from .openapi.api.apps import app_run_logs as _app_run_logs
from .openapi.api.apps import app_runs as _app_runs
from .openapi.api.apps import app_storage as _app_storage
from .openapi.api.apps import app_transfer as _app_transfer
from .openapi.api.apps import app_upgrade as _app_upgrade
from .openapi.api.apps import app_upgrade_unit as _app_upgrade_unit
from .openapi.api.apps import app_usage as _app_usage
from .openapi.api.apps import app_xcalls as _app_xcalls
from .openapi.api.apps import blob_delete as _blob_delete
from .openapi.api.apps import blob_download as _blob_download
from .openapi.api.apps import blob_info as _blob_info
from .openapi.api.apps import blob_list as _blob_list
from .openapi.api.apps import blob_revoke as _blob_revoke
from .openapi.api.apps import blob_upload as _blob_upload
from .openapi.api.apps import exports_export as _exports_export
from .openapi.api.apps import exports_segment as _exports_segment
from .openapi.api.apps import file_info as _file_info
from .openapi.api.apps import files_list as _files_list
from .openapi.api.apps import files_upload as _files_upload
from .openapi.api.apps import intents as _intents
from .openapi.api.apps import pipeline_receipt_receipt as _pipeline_receipt_receipt
from .openapi.api.apps import pipelines_cancel as _pipelines_cancel
from .openapi.api.apps import pipelines_estimate_run as _pipelines_estimate_run
from .openapi.api.apps import pipelines_get as _pipelines_get
from .openapi.api.apps import pipelines_list as _pipelines_list
from .openapi.api.apps import pipelines_retry as _pipelines_retry
from .openapi.api.apps import pipelines_start as _pipelines_start
from .openapi.api.apps import pipelines_validate as _pipelines_validate
from .openapi.api.apps import presence_get as _presence_get
from .openapi.api.apps import presence_publish as _presence_publish
from .openapi.api.apps import upload_abort as _upload_abort
from .openapi.api.apps import upload_begin as _upload_begin
from .openapi.api.apps import upload_complete as _upload_complete
from .openapi.api.apps import upload_part as _upload_part
from .openapi.api.apps import upload_status as _upload_status
from .openapi.api.apps import views as _views
from .openapi.api.consent import consent_page_create as _consent_page_create
from .openapi.api.consent import consent_page_result as _consent_page_result
from .openapi.api.ops import ops_apps as _ops_apps
from .openapi.api.ops import ops_backup as _ops_backup
from .openapi.api.ops import ops_backup_key as _ops_backup_key
from .openapi.api.ops import ops_cluster as _ops_cluster
from .openapi.api.ops import ops_cluster_owner as _ops_cluster_owner
from .openapi.api.ops import ops_deployer_grant as _ops_deployer_grant
from .openapi.api.ops import ops_deployer_revoke_delete as _ops_deployer_revoke_delete
from .openapi.api.ops import ops_deployer_revoke_post as _ops_deployer_revoke_post
from .openapi.api.ops import ops_deployments_granted as _ops_deployments_granted
from .openapi.api.ops import ops_job_node_attestation as _ops_job_node_attestation
from .openapi.api.ops import ops_metrics as _ops_metrics
from .openapi.api.ops import ops_org_upgrade as _ops_org_upgrade
from .openapi.api.ops import ops_orgs as _ops_orgs
from .openapi.api.ops import ops_outbound as _ops_outbound
from .openapi.api.ops import ops_package_install as _ops_package_install
from .openapi.api.ops import ops_package_list as _ops_package_list
from .openapi.api.ops import ops_package_uninstall as _ops_package_uninstall
from .openapi.api.ops import ops_package_upload as _ops_package_upload
from .openapi.api.ops import (
    ops_package_version_uninstall as _ops_package_version_uninstall,
)
from .openapi.api.ops import ops_placement as _ops_placement
from .openapi.api.ops import ops_pools_get as _ops_pools_get
from .openapi.api.ops import ops_pools_set as _ops_pools_set
from .openapi.api.ops import ops_prices_get as _ops_prices_get
from .openapi.api.ops import ops_prices_set as _ops_prices_set
from .openapi.api.ops import ops_quotas_get as _ops_quotas_get
from .openapi.api.ops import ops_quotas_set as _ops_quotas_set
from .openapi.api.ops import ops_receipt_key as _ops_receipt_key
from .openapi.api.ops import ops_recover as _ops_recover
from .openapi.api.ops import ops_recovery_files as _ops_recovery_files
from .openapi.api.ops import ops_register_node as _ops_register_node
from .openapi.api.ops import ops_release as _ops_release
from .openapi.api.ops import ops_release_deadline as _ops_release_deadline
from .openapi.api.ops import ops_release_deadline_lift as _ops_release_deadline_lift
from .openapi.api.ops import ops_remove_node as _ops_remove_node
from .openapi.api.ops import ops_restore as _ops_restore
from .openapi.api.ops import ops_served as _ops_served
from .openapi.api.ops import ops_set_backup_key as _ops_set_backup_key
from .openapi.api.ops import ops_set_replicas as _ops_set_replicas
from .openapi.api.ops import ops_sign_up_invite_create as _ops_sign_up_invite_create
from .openapi.api.ops import ops_sign_up_invite_list as _ops_sign_up_invite_list
from .openapi.api.ops import (
    ops_sign_up_invite_revoke_delete as _ops_sign_up_invite_revoke_delete,
)
from .openapi.api.ops import (
    ops_sign_up_invite_revoke_post as _ops_sign_up_invite_revoke_post,
)
from .openapi.api.ops import ops_sign_up_set as _ops_sign_up_set
from .openapi.api.ops import ops_status as _ops_status
from .openapi.api.ops import ops_upgrade as _ops_upgrade
from .openapi.api.organizations import app_team_list as _app_team_list
from .openapi.api.organizations import app_teams_scope as _app_teams_scope
from .openapi.api.organizations import invite_create as _invite_create
from .openapi.api.organizations import invite_list as _invite_list
from .openapi.api.organizations import invite_redeem as _invite_redeem
from .openapi.api.organizations import invite_revoke as _invite_revoke
from .openapi.api.organizations import member_metadata as _member_metadata
from .openapi.api.organizations import member_remove_teams as _member_remove_teams
from .openapi.api.organizations import member_set_teams as _member_set_teams
from .openapi.api.organizations import org_all_apps as _org_all_apps
from .openapi.api.organizations import org_apps as _org_apps
from .openapi.api.organizations import org_backup_key as _org_backup_key
from .openapi.api.organizations import org_capabilities as _org_capabilities
from .openapi.api.organizations import org_create as _org_create
from .openapi.api.organizations import org_delete as _org_delete
from .openapi.api.organizations import org_get as _org_get
from .openapi.api.organizations import org_members as _org_members
from .openapi.api.organizations import org_metadata as _org_metadata
from .openapi.api.organizations import org_outbound_approval as _org_outbound_approval
from .openapi.api.organizations import org_publisher as _org_publisher
from .openapi.api.organizations import org_release_approval as _org_release_approval
from .openapi.api.organizations import org_remove_member as _org_remove_member
from .openapi.api.organizations import org_rename as _org_rename
from .openapi.api.organizations import org_set_backup_key as _org_set_backup_key
from .openapi.api.organizations import org_set_member as _org_set_member
from .openapi.api.organizations import org_upgrade as _org_upgrade
from .openapi.api.organizations import org_upgrade_retry as _org_upgrade_retry
from .openapi.api.organizations import org_upgrade_status as _org_upgrade_status
from .openapi.api.organizations import org_usage as _org_usage
from .openapi.api.organizations import publisher_upgrade as _publisher_upgrade
from .openapi.api.organizations import recovery_cancel as _recovery_cancel
from .openapi.api.organizations import recovery_finish as _recovery_finish
from .openapi.api.organizations import recovery_set as _recovery_set
from .openapi.api.organizations import recovery_start as _recovery_start
from .openapi.api.organizations import team_create as _team_create
from .openapi.api.organizations import team_delete as _team_delete
from .openapi.api.organizations import team_get as _team_get
from .openapi.api.organizations import team_remove_member as _team_remove_member
from .openapi.api.organizations import team_set_member as _team_set_member
from .openapi.api.organizations import team_update as _team_update
from .openapi.api.packages import bundle_put as _bundle_put
from .openapi.api.packages import origins_add as _origins_add
from .openapi.api.packages import origins_list as _origins_list
from .openapi.api.packages import origins_remove as _origins_remove
from .openapi.api.packages import package_capabilities as _package_capabilities
from .openapi.api.packages import package_get as _package_get
from .openapi.api.packages import package_install as _package_install
from .openapi.api.packages import package_list as _package_list
from .openapi.api.packages import package_pipelines as _package_pipelines
from .openapi.api.packages import package_uninstall as _package_uninstall
from .openapi.api.packages import package_upload as _package_upload
from .openapi.api.packages import (
    package_version_uninstall as _package_version_uninstall,
)
from .openapi.api.public import attestation as _attestation
from .openapi.api.public import health as _health
from .openapi.api.public import publisher_binding as _publisher_binding
from .openapi.api.public import releases_public as _releases_public
from .openapi.api.public import restore_recover_holder as _restore_recover_holder
from .openapi.api.sign_up_and_deployments import deployer_grant as _deployer_grant
from .openapi.api.sign_up_and_deployments import deployer_list as _deployer_list
from .openapi.api.sign_up_and_deployments import deployer_revoke as _deployer_revoke
from .openapi.api.sign_up_and_deployments import deployment_create as _deployment_create
from .openapi.api.sign_up_and_deployments import deployment_delete as _deployment_delete
from .openapi.api.sign_up_and_deployments import deployment_get as _deployment_get
from .openapi.api.sign_up_and_deployments import deployment_list as _deployment_list
from .openapi.api.sign_up_and_deployments import deployment_offer as _deployment_offer
from .openapi.api.sign_up_and_deployments import deployment_orgs as _deployment_orgs
from .openapi.api.sign_up_and_deployments import (
    deployment_withdraw as _deployment_withdraw,
)
from .openapi.api.sign_up_and_deployments import (
    sign_up_invite_create as _sign_up_invite_create,
)
from .openapi.api.sign_up_and_deployments import (
    sign_up_invite_list as _sign_up_invite_list,
)
from .openapi.api.sign_up_and_deployments import (
    sign_up_invite_revoke as _sign_up_invite_revoke,
)
from .openapi.api.sign_up_and_deployments import sign_up_public as _sign_up_public
from .openapi.api.sign_up_and_deployments import sign_up_set as _sign_up_set
from .openapi.models import AccountProofProveBody
from .openapi.models import AccountRecoveryBeginBody
from .openapi.models import AccountRecoveryFinishBody
from .openapi.models import AccountRecoveryStartBody
from .openapi.models import AccountSetBackupKeyBody
from .openapi.models import AliasSetBody
from .openapi.models import ApikeyCreateBody
from .openapi.models import AppAccessBody
from .openapi.models import AppCreateBody
from .openapi.models import AppMetadataBody
from .openapi.models import AppMethodBody
from .openapi.models import AppMoveBody
from .openapi.models import AppTeamsScopeBody
from .openapi.models import AppTransferBody
from .openapi.models import AppUpgradeBody
from .openapi.models import AppUpgradeUnitBody
from .openapi.models import AttestationPurpose
from .openapi.models import ConfirmOnly
from .openapi.models import ConsentPageCreateBodyType0
from .openapi.models import ConsentPageCreateBodyType1
from .openapi.models import ConsentPageCreateBodyType10
from .openapi.models import ConsentPageCreateBodyType11
from .openapi.models import ConsentPageCreateBodyType12
from .openapi.models import ConsentPageCreateBodyType13
from .openapi.models import ConsentPageCreateBodyType14
from .openapi.models import ConsentPageCreateBodyType15
from .openapi.models import ConsentPageCreateBodyType2
from .openapi.models import ConsentPageCreateBodyType3
from .openapi.models import ConsentPageCreateBodyType4
from .openapi.models import ConsentPageCreateBodyType5
from .openapi.models import ConsentPageCreateBodyType6
from .openapi.models import ConsentPageCreateBodyType7
from .openapi.models import ConsentPageCreateBodyType8
from .openapi.models import ConsentPageCreateBodyType9
from .openapi.models import DeployerGrantBody
from .openapi.models import DeploymentCreateBody
from .openapi.models import DeploymentOfferBody
from .openapi.models import ExportsExportBody
from .openapi.models import IntentsBody
from .openapi.models import InviteCreateBody
from .openapi.models import LoginBeginBody
from .openapi.models import LoginFinishBody
from .openapi.models import MemberMetadataBody
from .openapi.models import MemberRemoveTeamsBody
from .openapi.models import MemberSetTeamsBody
from .openapi.models import OpsClusterOwnerBody
from .openapi.models import OpsDeployerGrantBody
from .openapi.models import OpsOrgUpgradeBody
from .openapi.models import OpsOutboundBody
from .openapi.models import OpsPackageInstallBody
from .openapi.models import OpsPoolsSetBody
from .openapi.models import OpsPricesSetBody
from .openapi.models import OpsRecoverBody
from .openapi.models import OpsRegisterNodeBody
from .openapi.models import OpsReleaseBody
from .openapi.models import OpsReleaseDeadlineBody
from .openapi.models import OpsRestoreBody
from .openapi.models import OpsSetBackupKeyBody
from .openapi.models import OpsSetReplicasBody
from .openapi.models import OpsSignUpInviteCreateBody
from .openapi.models import OpsSignUpSetBody
from .openapi.models import OpsUpgradeBody
from .openapi.models import OrgAllAppsBody
from .openapi.models import OrgCapabilitiesBody
from .openapi.models import OrgCreateBody
from .openapi.models import OrgMembersDetail
from .openapi.models import OrgMetadataBody
from .openapi.models import OrgOutboundApprovalBody
from .openapi.models import OrgPublisherBody
from .openapi.models import OrgReleaseApprovalBody
from .openapi.models import OrgRenameBody
from .openapi.models import OrgSetBackupKeyBody
from .openapi.models import OrgSetMemberBody
from .openapi.models import OrgUpgradeBody
from .openapi.models import OrgUpgradeRetryBody
from .openapi.models import OriginsAddBody
from .openapi.models import OriginsRemoveBody
from .openapi.models import PackageInstallBody
from .openapi.models import PasskeyAddFinishBody
from .openapi.models import PasskeyLabelBody
from .openapi.models import PipelinesEstimateRunBody
from .openapi.models import PipelinesRetryBody
from .openapi.models import PipelinesStartBody
from .openapi.models import PipelinesValidateBody
from .openapi.models import PresencePublishBody
from .openapi.models import ProofOnly
from .openapi.models import PublisherUpgradeBody
from .openapi.models import QuotaChanges
from .openapi.models import RecoverySetBody
from .openapi.models import RecoveryStartBody
from .openapi.models import RefreshBody
from .openapi.models import RegisterBeginBody
from .openapi.models import RegisterFinishBody
from .openapi.models import RestoreRecoverHolderBody
from .openapi.models import SignUpInviteCreateBody
from .openapi.models import SignUpSetBody
from .openapi.models import TeamCreateBody
from .openapi.models import TeamSetMemberBody
from .openapi.models import TeamUpdateBody
from .openapi.models import UploadBeginBody
from .openapi.models import ViewsBody
from ._facade import UNSET, Group, Operation

_APP_ABI = Operation(
    _app_abi,
    "app_abi",
    body="none",
    answer="json",
)
_APP_ACCESS = Operation(
    _app_access,
    "app_access",
    body=AppAccessBody,
    answer="json",
)
_APP_ATTESTATION = Operation(
    _app_attestation,
    "app_attestation",
    body="none",
    answer="json",
)
_BLOB_DELETE = Operation(
    _blob_delete,
    "blob_delete",
    body="none",
    answer="json",
)
_BLOB_DOWNLOAD = Operation(
    _blob_download,
    "blob_download",
    body="none",
    answer="bytes",
)
_BLOB_INFO = Operation(
    _blob_info,
    "blob_info",
    body="none",
    answer="json",
)
_BLOB_LIST = Operation(
    _blob_list,
    "blob_list",
    body="none",
    answer="json",
)
_BLOB_REVOKE = Operation(
    _blob_revoke,
    "blob_revoke",
    body="none",
    answer="json",
)
_BLOB_UPLOAD = Operation(
    _blob_upload,
    "blob_upload",
    body="bytes",
    answer="json",
)
_INTENTS = Operation(
    _intents,
    "intents",
    body=IntentsBody,
    answer="json",
    confirm=True,
    retry_requires="idempotencyKey",
)
_APP_CREATE = Operation(
    _app_create,
    "app_create",
    body=AppCreateBody,
    answer="json",
)
_APP_DELETE = Operation(
    _app_delete,
    "app_delete",
    body="none",
    answer="json",
)
_EXPORTS_EXPORT = Operation(
    _exports_export,
    "exports_export",
    body=ExportsExportBody,
    answer="json",
)
_EXPORTS_SEGMENT = Operation(
    _exports_segment,
    "exports_segment",
    body="none",
    answer="bytes",
)
_FILE_INFO = Operation(
    _file_info,
    "file_info",
    body="none",
    answer="json",
)
_FILES_LIST = Operation(
    _files_list,
    "files_list",
    body="none",
    answer="json",
)
_FILES_UPLOAD = Operation(
    _files_upload,
    "files_upload",
    body="bytes",
    answer="json",
)
_APP_GET = Operation(
    _app_get,
    "app_get",
    body="none",
    answer="json",
)
_APP_LIST = Operation(
    _app_list,
    "app_list",
    body="none",
    answer="json",
)
_APP_METADATA = Operation(
    _app_metadata,
    "app_metadata",
    body=AppMetadataBody,
    answer="json",
)
_APP_METHOD = Operation(
    _app_method,
    "app_method",
    body=AppMethodBody,
    answer="json",
    confirm=True,
    retry_requires="idempotencyKey",
)
_APP_MOVE = Operation(
    _app_move, "app_move", body=AppMoveBody, answer="json", confirm=True
)
_APP_OPENAPI = Operation(
    _app_openapi,
    "app_openapi",
    body="none",
    answer="json",
)
_PIPELINE_RECEIPT_RECEIPT = Operation(
    _pipeline_receipt_receipt,
    "pipeline_receipt_receipt",
    body="none",
    answer="json",
)
_PIPELINES_CANCEL = Operation(
    _pipelines_cancel,
    "pipelines_cancel",
    body="none",
    answer="json",
)
_PIPELINES_ESTIMATE_RUN = Operation(
    _pipelines_estimate_run,
    "pipelines_estimate_run",
    body=PipelinesEstimateRunBody,
    answer="json",
)
_PIPELINES_GET = Operation(
    _pipelines_get,
    "pipelines_get",
    body="none",
    answer="json",
)
_PIPELINES_LIST = Operation(
    _pipelines_list,
    "pipelines_list",
    body="none",
    answer="json",
)
_PIPELINES_RETRY = Operation(
    _pipelines_retry,
    "pipelines_retry",
    body=PipelinesRetryBody,
    answer="json",
)
_PIPELINES_START = Operation(
    _pipelines_start,
    "pipelines_start",
    body=PipelinesStartBody,
    answer="json",
)
_PIPELINES_VALIDATE = Operation(
    _pipelines_validate,
    "pipelines_validate",
    body=PipelinesValidateBody,
    answer="json",
)
_PRESENCE_GET = Operation(
    _presence_get,
    "presence_get",
    body="none",
    answer="json",
)
_PRESENCE_PUBLISH = Operation(
    _presence_publish,
    "presence_publish",
    body=PresencePublishBody,
    answer="json",
)
_APP_RUN = Operation(
    _app_run,
    "app_run",
    body="none",
    answer="json",
)
_APP_RUN_LOGS = Operation(
    _app_run_logs,
    "app_run_logs",
    body="none",
    answer="json",
)
_APP_RUNS = Operation(
    _app_runs,
    "app_runs",
    body="none",
    answer="json",
)
_APP_STORAGE = Operation(
    _app_storage,
    "app_storage",
    body="none",
    answer="json",
)
_APP_TRANSFER = Operation(
    _app_transfer, "app_transfer", body=AppTransferBody, answer="json", confirm=True
)
_APP_UPGRADE = Operation(
    _app_upgrade, "app_upgrade", body=AppUpgradeBody, answer="json", confirm=True
)
_APP_UPGRADE_UNIT = Operation(
    _app_upgrade_unit,
    "app_upgrade_unit",
    body=AppUpgradeUnitBody,
    answer="json",
    confirm=True,
)
_UPLOAD_ABORT = Operation(
    _upload_abort,
    "upload_abort",
    body="none",
    answer="json",
)
_UPLOAD_BEGIN = Operation(
    _upload_begin,
    "upload_begin",
    body=UploadBeginBody,
    answer="json",
)
_UPLOAD_COMPLETE = Operation(
    _upload_complete,
    "upload_complete",
    body="none",
    answer="json",
)
_UPLOAD_PART = Operation(
    _upload_part,
    "upload_part",
    body="bytes",
    answer="json",
)
_UPLOAD_STATUS = Operation(
    _upload_status,
    "upload_status",
    body="none",
    answer="json",
)
_APP_USAGE = Operation(
    _app_usage,
    "app_usage",
    body="none",
    answer="json",
)
_VIEWS = Operation(_views, "views", body=ViewsBody, answer="json", repeatable=True)
_APP_XCALLS = Operation(
    _app_xcalls,
    "app_xcalls",
    body="none",
    answer="json",
)


class Apps(Group):
    """`client.apps`: the operations core's document groups as `apps`."""

    def abi(self, *, app):
        """An app's ABI (`GET /apps/{app}/abi`, `app_abi`).

        The ABI its bundle embeds: methods, events, errors and state. `abi` is the bundle's own document, free-form here: its format is the ABI's, not this API's.

        **Who:** whoever the app's access admits (a public app: anyone).

        Args:
            app: path

        Returns: AppAbiResponse200
        """
        return self._run(_APP_ABI, (app,), {}, None)

    def access(self, *, app, access):
        """Set who may reach an app (`POST /apps/{app}/access`, `app_access`).

        A package-bound session: only its own package's app, from one of its package's teams to another.

        **Who:** a passkey session (an API key is refused).

        Args:
            app: path
            access: body `access`

        Returns: AppAccessResponse200
        """
        return self._run(_APP_ACCESS, (app,), {}, {"access": access})

    def attestation(self, *, app, nonce):
        """Every replica's receipt-key attestation (`GET /apps/{app}/attestation`, `app_attestation`).

        Each replica's `/attestation?purpose=receipts` for `nonce` (32 bytes, hex), or why it could not be had (`error`), and `cluster`, the cluster id each quote binds (with the replica's `node`): the documents are fetched in the clear, so their own `cluster` and `node` fields count for nothing. Any other query parameter is ignored.

        **Who:** whoever the app's access admits (a public app: anyone).

        Args:
            app: path
            nonce: query

        Returns: AppAttestationResponse200
        """
        return self._run(_APP_ATTESTATION, (app,), {"nonce": nonce}, None)

    def blob_delete(self, *, app, id):
        """Forget a blob (`DELETE /apps/{app}/blobs/{id}`, `blob_delete`).

        The owner or an organization admin.

        **Who:** whoever the app's access admits (a public app: anyone).

        Args:
            app: path
            id: path

        Returns: BlobDeleteResponse200
        """
        return self._run(
            _BLOB_DELETE,
            (
                app,
                id,
            ),
            {},
            None,
        )

    def blob_download(self, *, app, id, grant=UNSET):
        """Download a blob (`GET /apps/{app}/blobs/{id}`, `blob_download`).

        `{id}` is base58 or hex; `grant` is a link the app made (`BlobRef::grant`).

        **Who:** whoever the app's access admits (a public app: anyone).

        Args:
            app: path
            id: path
            grant: query

        Returns: File
        """
        return self._run(
            _BLOB_DOWNLOAD,
            (
                app,
                id,
            ),
            {"grant": grant},
            None,
        )

    def blob_info(self, *, app, id):
        """A blob's size (`GET /apps/{app}/blobs/{id}/info`, `blob_info`).

        **Who:** whoever the app's access admits (a public app: anyone).

        Args:
            app: path
            id: path

        Returns: BlobInfoResponse200
        """
        return self._run(
            _BLOB_INFO,
            (
                app,
                id,
            ),
            {},
            None,
        )

    def blob_list(self, *, app):
        """List an app's blobs (`GET /apps/{app}/blobs`, `blob_list`).

        Plain blobs only.

        **Who:** whoever the app's access admits (a public app: anyone).

        Args:
            app: path

        Returns: BlobListResponse200
        """
        return self._run(_BLOB_LIST, (app,), {}, None)

    def blob_revoke(self, *, app, id):
        """Invalidate a blob's grants (`POST /apps/{app}/blobs/{id}/revoke`, `blob_revoke`).

        The grants made so far for a published blob; the owner or an organization admin.

        **Who:** whoever the app's access admits (a public app: anyone).

        Args:
            app: path
            id: path

        Returns: BlobRevokeResponse200
        """
        return self._run(
            _BLOB_REVOKE,
            (
                app,
                id,
            ),
            {},
            None,
        )

    def blob_upload(
        self, *, app, sealed=UNSET, body, content_type="application/octet-stream"
    ):
        """Upload a blob (`POST /apps/{app}/blobs`, `blob_upload`).

        A plain blob, stored with the request's `Content-Type`, which a component app's download answers with: any media type (printable ASCII, 127 bytes at most; another is `400 BAD_REQUEST`), `application/octet-stream` when the request names none. Send the blob's own type, not `*/*`, which would be stored as its type. Any `sealed` parameter, `true` or `false`, is refused `400 USE_FILES_API`: sealed files go through `/files`.

        **Who:** whoever the app's access admits (a public app: anyone).

        Args:
            app: path
            sealed: query
            body: the raw bytes
            content_type: the body's media type

        Returns: BlobUploadResponse200
        """
        return self._run(
            _BLOB_UPLOAD, (app,), {"sealed": sealed}, body, content_type=content_type
        )

    def call(self, *, app, method, args=UNSET, idempotency_key=UNSET, confirm=None):
        """Call a method (`POST /apps/{app}/intents`, `intents`).

        A logged call: the result, the events of the call the caller receives, and the answering node's receipt. A `confirm` method answers `412` first. An app error is `422 APP_ERROR` with `details { index, code, fields }`; a trap `422 TRAP`. `args`, `result` and each event's `data` are the app's own JSON, free-form here: the app's ABI (`GET /apps/{app}/abi`, or its OpenAPI document) types them.

        **Who:** whoever the app's access admits (a public app: anyone). **Step-up:** when the method is a `confirm` method of the app's ABI, answers `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over `details.challenge`.

        Args:
            app: path
            method: body `method`
            args: body `args`
            idempotency_key: body `idempotencyKey`
            confirm: a passkey for the step-up, a ready `confirm`, or False

        Returns: IntentsResponse200
        """
        return self._run(
            _INTENTS,
            (app,),
            {},
            {"method": method, "args": args, "idempotencyKey": idempotency_key},
            confirm=confirm,
        )

    def create(
        self,
        *,
        name=UNSET,
        bundle=UNSET,
        package=UNSET,
        version=UNSET,
        service=UNSET,
        org=UNSET,
        access=UNSET,
        init_args=UNSET,
        metadata=UNSET,
        deployment=UNSET,
    ):
        """Create an app instance (`POST /apps`, `app_create`).

        From a release (`package`, `version`, `service`), a deployment's offer (`deployment`), or an uploaded `bundle`; in an organization (`org`), or, naming none, the caller's personal organization. Every app is in an organization. `initArgs` go to the app's `init` and `metadata` is kept as given (both free-form JSON). Answers once `init` committed; `compute` says whether the app has a compute component.

        **Who:** an installer (an account holder, or an unscoped API key that is not read-only).

        Args:
            name: body `name`
            bundle: body `bundle`
            package: body `package`
            version: body `version`
            service: body `service`
            org: body `org`
            access: body `access`
            init_args: body `initArgs`
            metadata: body `metadata`
            deployment: body `deployment`

        Returns: AppCreateResponse200
        """
        return self._run(
            _APP_CREATE,
            (),
            {},
            {
                "name": name,
                "bundle": bundle,
                "package": package,
                "version": version,
                "service": service,
                "org": org,
                "access": access,
                "initArgs": init_args,
                "metadata": metadata,
                "deployment": deployment,
            },
        )

    def delete(self, *, app):
        """Delete an app (`DELETE /apps/{app}`, `app_delete`).

        The owner or an organization admin.

        **Who:** a passkey session (an API key is refused).

        Args:
            app: path

        Returns: AppDeleteResponse200
        """
        return self._run(_APP_DELETE, (app,), {}, None)

    def exports_export(self, *, app, id, recipient):
        """Export a sealed file's key (`POST /apps/{app}/files/{id}/export`, `exports_export`).

        The owner and the organization's members. `recipient` is an X25519 public key (64 hex characters); the file's data key comes back wrapped to it, with the file's `segments` (each one's hash, hex) and `ephemeral` public key. The node never answers plaintext or a key in the clear. 30 a minute per account.

        **Who:** signed in (a session or an API key).

        Args:
            app: path
            id: path
            recipient: body `recipient`

        Returns: ExportsExportResponse200
        """
        return self._run(
            _EXPORTS_EXPORT,
            (
                app,
                id,
            ),
            {},
            {"recipient": recipient},
        )

    def exports_segment(self, *, app, id, k):
        """One segment of a sealed file (`GET /apps/{app}/files/{id}/segments/{k}`, `exports_segment`).

        Segment `k` (from 0) of the file's ciphertext, raw; the owner and the organization's members.

        **Who:** signed in (a session or an API key).

        Args:
            app: path
            id: path
            k: path

        Returns: File
        """
        return self._run(
            _EXPORTS_SEGMENT,
            (
                app,
                id,
                k,
            ),
            {},
            None,
        )

    def file_info(self, *, app, id):
        """One sealed file (`GET /apps/{app}/files/{id}`, `file_info`).

        Its plaintext size and SHA-256, never its key.

        **Who:** signed in (a session or an API key).

        Args:
            app: path
            id: path

        Returns: FileInfoResponse200
        """
        return self._run(
            _FILE_INFO,
            (
                app,
                id,
            ),
            {},
            None,
        )

    def files_list(self, *, app):
        """List an app's sealed files (`GET /apps/{app}/files`, `files_list`).

        **Who:** signed in (a session or an API key).

        Args:
            app: path

        Returns: FilesListResponse200
        """
        return self._run(_FILES_LIST, (app,), {}, None)

    def files_upload(self, *, app, body):
        """Upload a sealed file (`POST /apps/{app}/files`, `files_upload`).

        Up to 16 MiB in one request; the owner and the organization's members.

        **Who:** signed in (a session or an API key).

        Args:
            app: path
            body: the raw bytes

        Returns: FilesUploadResponse200
        """
        return self._run(_FILES_UPLOAD, (app,), {}, body)

    def get(self, *, app):
        """One app (`GET /apps/{app}`, `app_get`).

        `{app}` is an app id or its context id (`hex(SHA-256(app_id))`), here and on every `/apps/{app}` route.

        **Who:** signed in (a session or an API key).

        Args:
            app: path

        Returns: App
        """
        return self._run(_APP_GET, (app,), {}, None)

    def list(self, *, package=UNSET, bundle=UNSET):
        """Apps the caller reaches (`GET /apps`, `app_list`).

        Anyone may list public apps.

        **Who:** signed in (a session or an API key).

        Args:
            package: query
            bundle: query

        Returns: AppListResponse200
        """
        return self._run(_APP_LIST, (), {"package": package, "bundle": bundle}, None)

    def metadata(self, *, app, metadata):
        """Set an app's metadata (`POST /apps/{app}/metadata`, `app_metadata`).

        The owner or an organization admin; 16 KiB at most. `metadata` is the caller's own JSON, free-form, kept as given.

        **Who:** signed in (a session or an API key).

        Args:
            app: path
            metadata: body `metadata`

        Returns: AppMetadataResponse200
        """
        return self._run(_APP_METADATA, (app,), {}, {"metadata": metadata})

    def method(
        self,
        *,
        app,
        method,
        args=UNSET,
        idempotency_key=UNSET,
        consistency=UNSET,
        min_index=UNSET,
        confirm=None,
    ):
        """Call a method by its path (`POST /apps/{app}/methods/{method}`, `app_method`).

        The body of `/intents` or `/views` without `method`, which the path names, as the app's OpenAPI document lists it: a view the running bundle's ABI declares (without `confirm`) is answered as `/views`, anything else as `/intents`, with that route's gate, step-up and limits. Retry-safe as a view always, as a call with `idempotencyKey`. `args`, `result` and each event's `data` are the app's own JSON, free-form here: the app's ABI (`GET /apps/{app}/abi`, or its OpenAPI document) types them.

        **Who:** whoever the app's access admits (a public app: anyone). **Step-up:** when the method is a `confirm` method of the app's ABI, answers `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over `details.challenge`.

        Args:
            app: path
            method: path
            args: body `args`
            idempotency_key: body `idempotencyKey`
            consistency: body `consistency`
            min_index: body `minIndex`
            confirm: a passkey for the step-up, a ready `confirm`, or False

        Returns: AppMethodResponse200
        """
        return self._run(
            _APP_METHOD,
            (
                app,
                method,
            ),
            {},
            {
                "args": args,
                "idempotencyKey": idempotency_key,
                "consistency": consistency,
                "minIndex": min_index,
            },
            confirm=confirm,
        )

    def move(self, *, app, to_org, bring_members=UNSET, confirm=None):
        """Move an app to another organization (`POST /apps/{app}/move`, `app_move`).

        Its data where it is; its files, runs and quotas go to the target, `org` or app-team access becomes the target's app team of its package. `bringMembers` adds the source's members who reach it, capped at `member`. `from` is the organization it left, `null` for an app of the caller's own; `added` the accounts brought; `revokedInvites` how many invites to its old team were withdrawn.

        **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over `details.challenge`.

        Args:
            app: path
            to_org: body `toOrg`
            bring_members: body `bringMembers`
            confirm: a passkey for the step-up, a ready `confirm`, or False

        Returns: AppMoveResponse200
        """
        return self._run(
            _APP_MOVE,
            (app,),
            {},
            {"toOrg": to_org, "bringMembers": bring_members},
            confirm=confirm,
        )

    def openapi(self, *, app):
        """An app's OpenAPI document (`GET /apps/{app}/openapi.json`, `app_openapi`).

        Generated from the ABI of the bundle the app runs, under the access `/abi` has: one `POST /apps/{app}/methods/{method}` per method, with its arguments, result and errors. Free-form here: it is a document of its own, which a client reads as OpenAPI.

        **Who:** whoever the app's access admits (a public app: anyone).

        Args:
            app: path

        Returns: AppOpenapiResponse200
        """
        return self._run(_APP_OPENAPI, (app,), {}, None)

    def pipeline_receipt_receipt(self, *, app, run, version=UNSET):
        """A pipeline run's receipt (`GET /apps/{app}/pipeline-runs/{run}/receipt`, `pipeline_receipt_receipt`).

        The owner and the organization's members. For an ended run: the receipt, the answering node and its signature over it, by the node's receipt key. `version=2` adds what each job's report signs to its proof (`job`) and each job node's attestation (`jobAttestations`); the default, `1`, leaves them out.

        **Who:** signed in (a session or an API key).

        Args:
            app: path
            run: path
            version: query

        Returns: PipelineReceiptReceiptResponse200
        """
        return self._run(
            _PIPELINE_RECEIPT_RECEIPT,
            (
                app,
                run,
            ),
            {"version": version},
            None,
        )

    def pipelines_cancel(self, *, app, run):
        """Cancel a pipeline run (`POST /apps/{app}/pipeline-runs/{run}/cancel`, `pipelines_cancel`).

        The owner and the organization's members. Its pending steps end cancelled and its running steps' runs stop.

        **Who:** signed in (a session or an API key).

        Args:
            app: path
            run: path

        Returns: PipelinesCancelResponse200
        """
        return self._run(
            _PIPELINES_CANCEL,
            (
                app,
                run,
            ),
            {},
            None,
        )

    def pipelines_estimate_run(
        self, *, app, name, inputs=UNSET, counts=UNSET, params=UNSET
    ):
        """Estimate a pipeline run (`POST /apps/{app}/pipelines/{name}/estimate`, `pipelines_estimate_run`).

        The owner and the organization's members. Runs, fuel and cost of a run of a pipeline the app's release registers, for `inputs` (file ids, only counted) or `counts`, not both; numbers as decimal strings.

        **Who:** signed in (a session or an API key).

        Args:
            app: path
            name: path
            inputs: body `inputs`
            counts: body `counts`
            params: body `params`

        Returns: PipelinesEstimateRunResponse200
        """
        return self._run(
            _PIPELINES_ESTIMATE_RUN,
            (
                app,
                name,
            ),
            {},
            {"inputs": inputs, "counts": counts, "params": params},
        )

    def pipelines_get(self, *, app, run):
        """One pipeline run (`GET /apps/{app}/pipeline-runs/{run}`, `pipelines_get`).

        Its record: `state`, `version`, `hash`, `computeBundle`, `inputs`, `params`, per-step `status`, `runs`, `outputs`, `result`, `reason` and `cost` (`cost` and `costSoFar` for a run with a cap). Sizes, hashes and results only for callers who may list the app's files.

        **Who:** whoever the app's access admits (a public app: anyone).

        Args:
            app: path
            run: path

        Returns: PipelinesGetResponse200
        """
        return self._run(
            _PIPELINES_GET,
            (
                app,
                run,
            ),
            {},
            None,
        )

    def pipelines_list(self, *, app, limit=UNSET):
        """An app's pipeline runs (`GET /apps/{app}/pipeline-runs`, `pipelines_list`).

        The recent runs, newest first, as `{ runs }`; `limit` 1 to 100. Sizes, hashes and results only for callers who may list the app's files.

        **Who:** whoever the app's access admits (a public app: anyone).

        Args:
            app: path
            limit: query

        Returns: PipelinesListResponse200
        """
        return self._run(_PIPELINES_LIST, (app,), {"limit": limit}, None)

    def pipelines_retry(self, *, app, run, limits=UNSET, max_cost=UNSET):
        """Retry a pipeline run (`POST /apps/{app}/pipeline-runs/{run}/retry`, `pipelines_retry`).

        The owner and the organization's members. A new run of an ended run's pipeline at the same release, hash and compute component, with the same inputs and params, the `limits` applied and `retryOf` set. `202`.

        **Who:** signed in (a session or an API key).

        Args:
            app: path
            run: path
            limits: body `limits`
            max_cost: body `maxCost`

        Returns: PipelinesRetryResponse200
        """
        return self._run(
            _PIPELINES_RETRY,
            (
                app,
                run,
            ),
            {},
            {"limits": limits, "maxCost": max_cost},
        )

    def pipelines_start(self, *, app, name, inputs=UNSET, params=UNSET, max_cost=UNSET):
        """Start a pipeline run (`POST /apps/{app}/pipelines/{name}/runs`, `pipelines_start`).

        The owner and the organization's members. Starts a run of a pipeline the app's release registers. `maxCost` (micro-units, a decimal string) caps the run, and `worstCase` answers it. `202`.

        **Who:** signed in (a session or an API key).

        Args:
            app: path
            name: path
            inputs: body `inputs`
            params: body `params`
            max_cost: body `maxCost`

        Returns: PipelinesStartResponse200
        """
        return self._run(
            _PIPELINES_START,
            (
                app,
                name,
            ),
            {},
            {"inputs": inputs, "params": params, "maxCost": max_cost},
        )

    def pipelines_validate(self, *, source, package=UNSET, version=UNSET):
        """Validate a pipeline (`POST /pipelines/validate`, `pipelines_validate`).

        Every problem of `source`; with `package` and `version` (both, a release the caller sees) also against that release's methods, modules and the cluster's pools. Writes nothing.

        **Who:** signed in (a session or an API key).

        Args:
            source: body `source`
            package: body `package`
            version: body `version`

        Returns: PipelinesValidateResponse200
        """
        return self._run(
            _PIPELINES_VALIDATE,
            (),
            {},
            {"source": source, "package": package, "version": version},
        )

    def presence_get(self, *, app):
        """Who is present (`GET /apps/{app}/presence`, `presence_get`).

        One entry per present device (a session's passkey, or an API key): its account, device and state (hex).

        **Who:** whoever the app's access admits (a public app: anyone).

        Args:
            app: path

        Returns: PresenceGetResponse200
        """
        return self._run(_PRESENCE_GET, (app,), {}, None)

    def presence_publish(self, *, app, seq, state=UNSET):
        """Publish presence (`POST /apps/{app}/presence`, `presence_publish`).

        The caller's presence state (hex), in sequence; not logged. A `null` (or absent) `state` retracts it.

        **Who:** whoever the app's access admits (a public app: anyone).

        Args:
            app: path
            seq: body `seq`
            state: body `state`

        Returns: PresencePublishResponse200
        """
        return self._run(_PRESENCE_PUBLISH, (app,), {}, {"seq": seq, "state": state})

    def run(self, *, app, run):
        """One compute run (`GET /apps/{app}/runs/{run}`, `app_run`).

        Its record without arguments or keys: method, state, `cancelRequested`, `startTsMs`, attempts, input and output ids (sizes and SHA-256 only for callers who may list the app's files), usage, and `progress` as the leader last polled it.

        **Who:** whoever the app's access admits (a public app: anyone).

        Args:
            app: path
            run: path

        Returns: Run
        """
        return self._run(
            _APP_RUN,
            (
                app,
                run,
            ),
            {},
            None,
        )

    def run_logs(self, *, app, run):
        """A compute run's attempt logs (`GET /apps/{app}/runs/{run}/logs`, `app_run_logs`).

        Each attempt's module log, as a sealed file of the app: `{ attempt, id, size, sha256 }`. Only for those who may handle the app's files; the bytes are a files export.

        **Who:** signed in (a session or an API key).

        Args:
            app: path
            run: path

        Returns: AppRunLogsResponse200
        """
        return self._run(
            _APP_RUN_LOGS,
            (
                app,
                run,
            ),
            {},
            None,
        )

    def runs(self, *, app, limit=UNSET, before=UNSET):
        """An app's compute runs (`GET /apps/{app}/runs`, `app_runs`).

        Newest first, `limit` (1 to 100, default 20) at a time; `next` is the `before` of the next page, `null` on the last.

        **Who:** whoever the app's access admits (a public app: anyone).

        Args:
            app: path
            limit: query
            before: query

        Returns: AppRunsResponse200
        """
        return self._run(_APP_RUNS, (app,), {"limit": limit, "before": before}, None)

    def storage(self, *, app):
        """What an app stores (`GET /apps/{app}/storage`, `app_storage`).

        Keys, bytes and blobs held, and `lastWriteAt` (Unix seconds of its last committed write, `null` before one).

        **Who:** whoever the app's access admits (a public app: anyone).

        Args:
            app: path

        Returns: AppStorageResponse200
        """
        return self._run(_APP_STORAGE, (app,), {}, None)

    def transfer(self, *, app, to, confirm=None):
        """Hand an app to another account (`POST /apps/{app}/transfer`, `app_transfer`).

        The owner (for an organization's app, while a member and in the app's team), or an organization admin; an organization's app only to a member. Answers `null`.

        **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over `details.challenge`.

        Args:
            app: path
            to: body `to`
            confirm: a passkey for the step-up, a ready `confirm`, or False

        Returns: None
        """
        return self._run(_APP_TRANSFER, (app,), {}, {"to": to}, confirm=confirm)

    def upgrade(
        self,
        *,
        app,
        bundle=UNSET,
        package=UNSET,
        version=UNSET,
        service=UNSET,
        migrate=UNSET,
        confirm=None,
    ):
        """Upgrade an app instance (`POST /apps/{app}/upgrade`, `app_upgrade`).

        To a release (`version`, of the app's own package unless `package` names another) or a `bundle`, code only: the state stays and is read by the new code. Whoever the package's manifest names (`upgrades`: the owner, or its publisher's `MANAGE_APPLICATION` holders), with a step-up over `{ bundle, migrate, manifest }`; an upgrade key signs in place of it. A refused or failed upgrade leaves the app on its release and answers `422 UPGRADE_FAILED` with the app log's error in `details` (`MigrationsNotSupported` when `migrate` is named: there are no migrations in this version). `error` is the app log's error value (`null` when it ran), free-form: the app's own.

        **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over `details.challenge`.

        Args:
            app: path
            bundle: body `bundle`
            package: body `package`
            version: body `version`
            service: body `service`
            migrate: body `migrate`
            confirm: a passkey for the step-up, a ready `confirm`, or False

        Returns: AppUpgradeResponse200
        """
        return self._run(
            _APP_UPGRADE,
            (app,),
            {},
            {
                "bundle": bundle,
                "package": package,
                "version": version,
                "service": service,
                "migrate": migrate,
            },
            confirm=confirm,
        )

    def upgrade_unit(self, *, app, version, confirm=None):
        """Upgrade an app and the apps it created, in one consent (`POST /apps/{app}/upgrade-unit`, `app_upgrade_unit`).

        An instance created with apps of its own (`links`): it moves to `version` of its package, and each member the target release's `uses` gives a release other than the one it runs moves to that release (its lock's, else the newest installed one inside the range), all under one step-up over `{ app, package, version, targets }`. Answers the run (`GET /orgs/{org}/upgrades/{run}` follows it); each app is upgraded on its own as an organization's run does. `400 NOT_A_UNIT` for an app with no members; `409 CONFLICT` when the target release creates a member the instance lacks.

        **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over `details.challenge`.

        Args:
            app: path
            version: body `version`
            confirm: a passkey for the step-up, a ready `confirm`, or False

        Returns: UpgradeRun
        """
        return self._run(
            _APP_UPGRADE_UNIT, (app,), {}, {"version": version}, confirm=confirm
        )

    def upload_abort(self, *, app, upload):
        """Abort an upload (`DELETE /apps/{app}/files/uploads/{upload}`, `upload_abort`).

        **Who:** signed in (a session or an API key).

        Args:
            app: path
            upload: path

        Returns: UploadAbortResponse200
        """
        return self._run(
            _UPLOAD_ABORT,
            (
                app,
                upload,
            ),
            {},
            None,
        )

    def upload_begin(self, *, app, size):
        """Begin a file upload (`POST /apps/{app}/files/uploads`, `upload_begin`).

        A session for a file of `size` bytes, sent in parts of `partBytes`.

        **Who:** signed in (a session or an API key).

        Args:
            app: path
            size: body `size`

        Returns: UploadBeginResponse200
        """
        return self._run(_UPLOAD_BEGIN, (app,), {}, {"size": size})

    def upload_complete(self, *, app, upload):
        """Commit an upload (`POST /apps/{app}/files/uploads/{upload}/complete`, `upload_complete`).

        Charges and commits the file; a retry answers the same file. `409 UPLOAD_INCOMPLETE` names the part still missing.

        **Who:** signed in (a session or an API key).

        Args:
            app: path
            upload: path

        Returns: UploadCompleteResponse200
        """
        return self._run(
            _UPLOAD_COMPLETE,
            (
                app,
                upload,
            ),
            {},
            None,
        )

    def upload_part(self, *, app, upload, k, body):
        """Send one part (`PUT /apps/{app}/files/uploads/{upload}/parts/{k}`, `upload_part`).

        Part `k` (from 0), raw bytes; a part out of order (`409 PART_OUT_OF_ORDER`), or one that differs from the part sent before as `k` (`409 PART_CONFLICT`), answers the next one expected in `details.next`.

        **Who:** signed in (a session or an API key).

        Args:
            app: path
            upload: path
            k: path
            body: the raw bytes

        Returns: UploadPartResponse200
        """
        return self._run(
            _UPLOAD_PART,
            (
                app,
                upload,
                k,
            ),
            {},
            body,
        )

    def upload_status(self, *, app, upload):
        """Where an upload is (`GET /apps/{app}/files/uploads/{upload}`, `upload_status`).

        **Who:** signed in (a session or an API key).

        Args:
            app: path
            upload: path

        Returns: UploadStatusResponse200
        """
        return self._run(
            _UPLOAD_STATUS,
            (
                app,
                upload,
            ),
            {},
            None,
        )

    def usage(self, *, app):
        """An app's runs per day (`GET /apps/{app}/usage`, `app_usage`).

        The owner or an organization admin: `today` and the 30 `days` before it, figures as decimal strings; `wallMs` is information only.

        **Who:** signed in (a session or an API key).

        Args:
            app: path

        Returns: AppUsageResponse200
        """
        return self._run(_APP_USAGE, (app,), {}, None)

    def view(self, *, app, method, args=UNSET, consistency=UNSET, min_index=UNSET):
        """Read with a view method (`POST /apps/{app}/views`, `views`).

        Read linearizably (`consistency: "strong"`, the default; any value but `"stale"` reads as it): the replica answering waits until it applied the index the app's leader names once it confirmed it leads, and sends the view to the leader if it cannot catch up; or (`"stale"`) on the replica answering, as it stands; `minIndex` waits until the replica applied it (`503 NOT_CAUGHT_UP`). `args`, `result` and each event's `data` are the app's own JSON, free-form here: the app's ABI (`GET /apps/{app}/abi`, or its OpenAPI document) types them.

        **Who:** whoever the app's access admits (a public app: anyone).

        Args:
            app: path
            method: body `method`
            args: body `args`
            consistency: body `consistency`
            min_index: body `minIndex`

        Returns: ViewsResponse200
        """
        return self._run(
            _VIEWS,
            (app,),
            {},
            {
                "method": method,
                "args": args,
                "consistency": consistency,
                "minIndex": min_index,
            },
        )

    def xcalls(self, *, app):
        """An app's cross-app calls (`GET /apps/{app}/xcalls`, `app_xcalls`).

        The owner or an organization admin: how many calls wait in its outbox for delivery (`pending`, a count), and its dead letters, oldest first (`atMs`: the entry time it was dead-lettered at, Unix milliseconds).

        **Who:** signed in (a session or an API key).

        Args:
            app: path

        Returns: AppXcallsResponse200
        """
        return self._run(_APP_XCALLS, (app,), {}, None)


_ACCOUNT_APPS = Operation(
    _account_apps,
    "account_apps",
    body="none",
    answer="json",
)
_ACCOUNT_BACKUP_KEY = Operation(
    _account_backup_key,
    "account_backup_key",
    body="none",
    answer="json",
)
_ACCOUNT_PROOF_PROVE = Operation(
    _account_proof_prove,
    "account_proof_prove",
    body=AccountProofProveBody,
    answer="json",
)
_ACCOUNT_RECOVERY_BEGIN = Operation(
    _account_recovery_begin,
    "account_recovery_begin",
    body=AccountRecoveryBeginBody,
    answer="json",
    once=True,
)
_ACCOUNT_RECOVERY_CANCEL = Operation(
    _account_recovery_cancel,
    "account_recovery_cancel",
    body=ConfirmOnly,
    answer="json",
    confirm=True,
)
_ACCOUNT_RECOVERY_FINISH = Operation(
    _account_recovery_finish,
    "account_recovery_finish",
    body=AccountRecoveryFinishBody,
    answer="json",
    confirm=True,
    once=True,
)
_ACCOUNT_RECOVERY_START = Operation(
    _account_recovery_start,
    "account_recovery_start",
    body=AccountRecoveryStartBody,
    answer="json",
    once=True,
)
_ACCOUNT_SET_BACKUP_KEY = Operation(
    _account_set_backup_key,
    "account_set_backup_key",
    body=AccountSetBackupKeyBody,
    answer="json",
    confirm=True,
)
_ACTIVITY_LIST = Operation(
    _activity_list,
    "activity_list",
    body="none",
    answer="json",
)
_ALIAS_DELETE = Operation(
    _alias_delete,
    "alias_delete",
    body="none",
    answer="json",
)
_ALIAS_GET = Operation(
    _alias_get,
    "alias_get",
    body="none",
    answer="json",
)
_ALIAS_LIST = Operation(
    _alias_list,
    "alias_list",
    body="none",
    answer="json",
)
_ALIAS_SET = Operation(
    _alias_set,
    "alias_set",
    body=AliasSetBody,
    answer="json",
)
_APIKEY_CREATE = Operation(
    _apikey_create, "apikey_create", body=ApikeyCreateBody, answer="json", confirm=True
)
_APIKEY_LIST = Operation(
    _apikey_list,
    "apikey_list",
    body="none",
    answer="json",
)
_APIKEY_REVOKE = Operation(
    _apikey_revoke, "apikey_revoke", body=ConfirmOnly, answer="json", confirm=True
)
_LOGIN_BEGIN = Operation(
    _login_begin, "login_begin", body=LoginBeginBody, answer="json", once=True
)
_LOGIN_FINISH = Operation(
    _login_finish, "login_finish", body=LoginFinishBody, answer="json", once=True
)
_LOGOUT = Operation(
    _logout,
    "logout",
    body="none",
    answer="json",
)
_LOGOUT_ALL = Operation(
    _logout_all,
    "logout_all",
    body="none",
    answer="json",
)
_ME = Operation(
    _me,
    "me",
    body="none",
    answer="json",
)
_PASSKEY_ADD_BEGIN = Operation(
    _passkey_add_begin,
    "passkey_add_begin",
    body="none",
    answer="json",
)
_PASSKEY_ADD_FINISH = Operation(
    _passkey_add_finish,
    "passkey_add_finish",
    body=PasskeyAddFinishBody,
    answer="json",
    confirm=True,
)
_PASSKEY_LABEL = Operation(
    _passkey_label,
    "passkey_label",
    body=PasskeyLabelBody,
    answer="json",
)
_PASSKEY_REMOVE = Operation(
    _passkey_remove, "passkey_remove", body=ConfirmOnly, answer="json", confirm=True
)
_RECOVERY_CREATE = Operation(
    _recovery_create, "recovery_create", body=ConfirmOnly, answer="json", confirm=True
)
_RECOVERY_REMOVE = Operation(
    _recovery_remove, "recovery_remove", body=ConfirmOnly, answer="json", confirm=True
)
_REFRESH = Operation(
    _refresh,
    "refresh",
    body=RefreshBody,
    answer="json",
)
_REGISTER_BEGIN = Operation(
    _register_begin, "register_begin", body=RegisterBeginBody, answer="json", once=True
)
_REGISTER_FINISH = Operation(
    _register_finish,
    "register_finish",
    body=RegisterFinishBody,
    answer="json",
    confirm=True,
    once=True,
)
_SESSION_REVOKE = Operation(
    _session_revoke,
    "session_revoke",
    body="none",
    answer="json",
)
_SESSIONS_LIST = Operation(
    _sessions_list,
    "sessions_list",
    body="none",
    answer="json",
)
_USAGE = Operation(
    _usage,
    "usage",
    body="none",
    answer="json",
)


class Auth(Group):
    """`client.auth`: the operations core's document groups as `auth`."""

    def account_apps(self, *, after=UNSET, limit=UNSET):
        """The account's own apps (`GET /auth/apps`, `account_apps`).

        The apps the account owns, in any organization (a package-bound session: its package's), with its personal organization's `approvedReleases`: each app's release, the newer releases it may upgrade to, its last upgrade, its replicas' releases and `lastWriteAt`.

        **Who:** signed in (a session or an API key).

        Args:
            after: query
            limit: query

        Returns: AccountAppsResponse200
        """
        return self._run(_ACCOUNT_APPS, (), {"after": after, "limit": limit}, None)

    def account_backup_key(self):
        """The account's backup key (`GET /auth/backup-key`, `account_backup_key`).

        The X25519 public key the recovery-disk backups of the account's personal apps are sealed to (`key`, `null` while none is set).

        **Who:** an account holder (a passkey session not bound to an app's frontend).

        Returns: BackupKeyAnswer
        """
        return self._run(_ACCOUNT_BACKUP_KEY, (), {}, None)

    def account_proof_prove(self, *, nonce, audience):
        """Prove the account to another party (`POST /auth/account-proof`, `account_proof_prove`).

        A statement signed with this node's receipt key that the caller's passkey session asked to bind `nonce` for `audience`.

        **Who:** a passkey session (an API key is refused).

        Args:
            nonce: body `nonce`
            audience: body `audience`

        Returns: AccountProof
        """
        return self._run(
            _ACCOUNT_PROOF_PROVE, (), {}, {"nonce": nonce, "audience": audience}
        )

    def account_recovery_begin(self, *, code):
        """Start an account recovery (`POST /auth/recovery/begin`, `account_recovery_begin`).

        Signed out, with the recovery code: creation options for the passkey that will replace the lost ones. `options` are the WebAuthn options to hand to the browser as they are: free-form here, their shape is WebAuthn's.

        **Who:** anyone.

        Args:
            code: body `code`

        Returns: AccountRecoveryBeginResponse200
        """
        return self._run(_ACCOUNT_RECOVERY_BEGIN, (), {}, {"code": code})

    def account_recovery_cancel(self, *, confirm=None):
        """Cancel an account recovery (`POST /auth/recovery/cancel`, `account_recovery_cancel`).

        With a passkey the account has; `412` first.

        **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over `details.challenge`.

        Args:
            confirm: a passkey for the step-up, a ready `confirm`, or False

        Returns: AccountRecoveryCancelResponse200
        """
        return self._run(_ACCOUNT_RECOVERY_CANCEL, (), {}, {}, confirm=confirm)

    def account_recovery_finish(self, *, account_id=UNSET, confirm=None):
        """Finish an account recovery (`POST /auth/recovery/finish`, `account_recovery_finish`).

        Approved by the new passkey once `readyAt` passed (`412` first): adds it and ends every session of the account.

        **Who:** anyone. **Step-up:** answers `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over `details.challenge`.

        Args:
            account_id: body `accountId`
            confirm: a passkey for the step-up, a ready `confirm`, or False

        Returns: Session
        """
        return self._run(
            _ACCOUNT_RECOVERY_FINISH, (), {}, {"accountId": account_id}, confirm=confirm
        )

    def account_recovery_start(self, *, challenge_id, code, registration):
        """Spend the recovery code (`POST /auth/recovery/start`, `account_recovery_start`).

        Logs the new passkey as pending; it may finish the recovery once `readyAt` passed (72 h). `401` for a wrong or spent code.

        **Who:** anyone.

        Args:
            challenge_id: body `challengeId`
            code: body `code`
            registration: body `registration`

        Returns: AccountRecoveryStartResponse200
        """
        return self._run(
            _ACCOUNT_RECOVERY_START,
            (),
            {},
            {"challengeId": challenge_id, "code": code, "registration": registration},
        )

    def account_set_backup_key(self, *, public_key, confirm=None):
        """Set the account's backup key (`POST /auth/backup-key`, `account_set_backup_key`).

        `publicKey`: 64 lowercase hex, an X25519 public key whose private half the account holder keeps (never a node), for its personal apps, where owners upgrade apps. `412` first.

        **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over `details.challenge`.

        Args:
            public_key: body `publicKey`
            confirm: a passkey for the step-up, a ready `confirm`, or False

        Returns: BackupKeyAnswer
        """
        return self._run(
            _ACCOUNT_SET_BACKUP_KEY, (), {}, {"publicKey": public_key}, confirm=confirm
        )

    def activity_list(self, *, after=UNSET):
        """What the account's passkeys did (`GET /auth/activity`, `activity_list`).

        Newest first, the last 100: sign-ins (`app`, `origin`) and approvals (`method`, `subject`); `after` keeps the entries after that Unix second.

        **Who:** an account holder (a passkey session not bound to an app's frontend).

        Args:
            after: query

        Returns: ActivityListResponse200
        """
        return self._run(_ACTIVITY_LIST, (), {"after": after}, None)

    def alias_delete(self, *, alias):
        """Remove an alias (`DELETE /auth/aliases/{alias}`, `alias_delete`).

        **Who:** signed in (a session or an API key).

        Args:
            alias: path

        Returns: AliasDeleteResponse200
        """
        return self._run(_ALIAS_DELETE, (alias,), {}, None)

    def alias_get(self, *, alias):
        """Resolve an alias (`GET /auth/aliases/{alias}`, `alias_get`).

        **Who:** signed in (a session or an API key).

        Args:
            alias: path

        Returns: AliasGetResponse200
        """
        return self._run(_ALIAS_GET, (alias,), {}, None)

    def alias_list(self):
        """The account's aliases (`GET /auth/aliases`, `alias_list`).

        **Who:** signed in (a session or an API key).

        Returns: AliasListResponse200
        """
        return self._run(_ALIAS_LIST, (), {}, None)

    def alias_set(self, *, alias, app):
        """Set an alias (`PUT /auth/aliases/{alias}`, `alias_set`).

        **Who:** signed in (a session or an API key).

        Args:
            alias: path
            app: body `app`

        Returns: AliasSetResponse200
        """
        return self._run(_ALIAS_SET, (alias,), {}, {"app": app})

    def apikey_create(
        self,
        *,
        label=UNSET,
        apps=UNSET,
        methods=UNSET,
        read_only=UNSET,
        allow_confirm=UNSET,
        ttl_secs=UNSET,
        upgrades=UNSET,
        confirm=None,
    ):
        """Make an API key (`POST /auth/apikeys`, `apikey_create`).

        Scoped to `apps` and `methods`, read-only or not, for `ttlSecs`; `upgrades` makes an upgrade key, which signs upgrades of one app in place of a passkey. `412` first. The key is shown once.

        **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over `details.challenge`.

        Args:
            label: body `label`
            apps: body `apps`
            methods: body `methods`
            read_only: body `readOnly`
            allow_confirm: body `allowConfirm`
            ttl_secs: body `ttlSecs`
            upgrades: body `upgrades`
            confirm: a passkey for the step-up, a ready `confirm`, or False

        Returns: ApikeyCreateResponse200
        """
        return self._run(
            _APIKEY_CREATE,
            (),
            {},
            {
                "label": label,
                "apps": apps,
                "methods": methods,
                "readOnly": read_only,
                "allowConfirm": allow_confirm,
                "ttlSecs": ttl_secs,
                "upgrades": upgrades,
            },
            confirm=confirm,
        )

    def apikey_list(self):
        """The account's API keys (`GET /auth/apikeys`, `apikey_list`).

        **Who:** an account holder (a passkey session not bound to an app's frontend).

        Returns: ApikeyListResponse200
        """
        return self._run(_APIKEY_LIST, (), {}, None)

    def apikey_revoke(self, *, key, confirm=None):
        """Revoke an API key (`DELETE /auth/apikeys/{key}`, `apikey_revoke`).

        **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over `details.challenge`.

        Args:
            key: path
            confirm: a passkey for the step-up, a ready `confirm`, or False

        Returns: ApikeyRevokeResponse200
        """
        return self._run(_APIKEY_REVOKE, (key,), {}, {}, confirm=confirm)

    def login_begin(self, *, origin=UNSET):
        """Start a sign-in (`POST /auth/passkey/login/begin`, `login_begin`).

        A WebAuthn challenge for any passkey of the cluster. `origin` is an app frontend's registered origin, whose rp-id the ceremony then uses. `options` are the WebAuthn options to hand to the browser as they are: free-form here, their shape is WebAuthn's.

        **Who:** anyone.

        Args:
            origin: body `origin`

        Returns: LoginBeginResponse200
        """
        return self._run(_LOGIN_BEGIN, (), {}, {"origin": origin})

    def login_finish(self, *, challenge_id, assertion):
        """Finish a sign-in (`POST /auth/passkey/login/finish`, `login_finish`).

        **Who:** anyone.

        Args:
            challenge_id: body `challengeId`
            assertion: body `assertion`

        Returns: Session
        """
        return self._run(
            _LOGIN_FINISH, (), {}, {"challengeId": challenge_id, "assertion": assertion}
        )

    def logout(self):
        """End this session (`POST /auth/logout`, `logout`).

        **Who:** a passkey session (an API key is refused).

        Returns: LogoutResponse200
        """
        return self._run(_LOGOUT, (), {}, None)

    def logout_all(self):
        """End every session of the account (`POST /auth/logout-all`, `logout_all`).

        `revoked`: how many sessions ended.

        **Who:** an account holder (a passkey session not bound to an app's frontend).

        Returns: LogoutAllResponse200
        """
        return self._run(_LOGOUT_ALL, (), {}, None)

    def me(self):
        """The caller's account (`GET /auth/me`, `me`).

        The account, its passkeys (with `addedAt` and `lastSignInAt`), organizations (`personal`: the one made at sign-up, where its instances go unless they name another; its approvals are that organization's), recovery state, and how the caller came (`via`).

        **Who:** signed in (a session or an API key).

        Returns: MeResponse200
        """
        return self._run(_ME, (), {}, None)

    def passkey_add_begin(self):
        """Start adding a passkey (`POST /auth/passkeys/add/begin`, `passkey_add_begin`).

        Creation options for another passkey (a device) on the caller's account. `options` are the WebAuthn options to hand to the browser as they are: free-form here, their shape is WebAuthn's.

        **Who:** an account holder (a passkey session not bound to an app's frontend).

        Returns: PasskeyAddBeginResponse200
        """
        return self._run(_PASSKEY_ADD_BEGIN, (), {}, None)

    def passkey_add_finish(self, *, challenge_id, registration, confirm=None):
        """Finish adding a passkey (`POST /auth/passkeys/add/finish`, `passkey_add_finish`).

        Answers `412` first; sent again with `confirm` signed by a passkey the account already has.

        **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over `details.challenge`.

        Args:
            challenge_id: body `challengeId`
            registration: body `registration`
            confirm: a passkey for the step-up, a ready `confirm`, or False

        Returns: PasskeyAddFinishResponse200
        """
        return self._run(
            _PASSKEY_ADD_FINISH,
            (),
            {},
            {"challengeId": challenge_id, "registration": registration},
            confirm=confirm,
        )

    def passkey_label(self, *, cred, label):
        """Name a passkey (`POST /auth/passkeys/{cred}`, `passkey_label`).

        **Who:** an account holder (a passkey session not bound to an app's frontend).

        Args:
            cred: path
            label: body `label`

        Returns: PasskeyLabelResponse200
        """
        return self._run(_PASSKEY_LABEL, (cred,), {}, {"label": label})

    def passkey_remove(self, *, cred, confirm=None):
        """Remove a passkey (`DELETE /auth/passkeys/{cred}`, `passkey_remove`).

        Its sessions end. `412` first; the retry's JSON body is `{ confirm }`.

        **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over `details.challenge`.

        Args:
            cred: path
            confirm: a passkey for the step-up, a ready `confirm`, or False

        Returns: PasskeyRemoveResponse200
        """
        return self._run(_PASSKEY_REMOVE, (cred,), {}, {}, confirm=confirm)

    def recovery_create(self, *, confirm=None):
        """Make a recovery code (`POST /auth/recovery`, `recovery_create`).

        Shown once, replacing any; `412` first.

        **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over `details.challenge`.

        Args:
            confirm: a passkey for the step-up, a ready `confirm`, or False

        Returns: RecoveryCreateResponse200
        """
        return self._run(_RECOVERY_CREATE, (), {}, {}, confirm=confirm)

    def recovery_remove(self, *, confirm=None):
        """Remove the recovery code (`DELETE /auth/recovery`, `recovery_remove`).

        **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over `details.challenge`.

        Args:
            confirm: a passkey for the step-up, a ready `confirm`, or False

        Returns: RecoveryRemoveResponse200
        """
        return self._run(_RECOVERY_REMOVE, (), {}, {}, confirm=confirm)

    def refresh(self, *, refresh_token):
        """Refresh a session (`POST /auth/refresh`, `refresh`).

        A new session and the refresh token to use next (the same one until it is an hour old). A reused, superseded token ends the sessions it opened.

        **Who:** anyone.

        Args:
            refresh_token: body `refreshToken`

        Returns: Session
        """
        return self._run(_REFRESH, (), {}, {"refreshToken": refresh_token})

    def register_begin(self, *, name=UNSET, origin=UNSET, invite=UNSET):
        """Start a sign-up (`POST /auth/passkey/register/begin`, `register_begin`).

        Creation options for a new passkey. `invite` is an organization or cluster sign-up invite token, which the policy may require; `403 SIGN_UP_REFUSED` when the policy would refuse the sign-up. `options` are the WebAuthn options to hand to the browser as they are: free-form here, their shape is WebAuthn's.

        **Who:** anyone.

        Args:
            name: body `name`
            origin: body `origin`
            invite: body `invite`

        Returns: RegisterBeginResponse200
        """
        return self._run(
            _REGISTER_BEGIN, (), {}, {"name": name, "origin": origin, "invite": invite}
        )

    def register_finish(
        self, *, challenge_id, registration, invite=UNSET, confirm=None
    ):
        """Finish a sign-up (`POST /auth/passkey/register/finish`, `register_finish`).

        The new passkey's registration. Answers `412 CONFIRM_REQUIRED` first: the new passkey signs the account it creates; the retry with `confirm` (and the same `invite`) opens a session, and redeems `invite` for the new account.

        **Who:** anyone. **Step-up:** answers `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over `details.challenge`.

        Args:
            challenge_id: body `challengeId`
            registration: body `registration`
            invite: body `invite`
            confirm: a passkey for the step-up, a ready `confirm`, or False

        Returns: Session
        """
        return self._run(
            _REGISTER_FINISH,
            (),
            {},
            {
                "challengeId": challenge_id,
                "registration": registration,
                "invite": invite,
            },
            confirm=confirm,
        )

    def session_revoke(self, *, session):
        """End one session (`DELETE /auth/sessions/{session}`, `session_revoke`).

        `404` for a session the account does not hold live.

        **Who:** an account holder (a passkey session not bound to an app's frontend).

        Args:
            session: path

        Returns: SessionRevokeResponse200
        """
        return self._run(_SESSION_REVOKE, (session,), {}, None)

    def sessions_list(self):
        """The account's live sessions (`GET /auth/sessions`, `sessions_list`).

        Latest active first, at most 256. `openedAt` (Unix seconds) is `null` for a session opened before the node kept it.

        **Who:** an account holder (a passkey session not bound to an app's frontend).

        Returns: SessionsListResponse200
        """
        return self._run(_SESSIONS_LIST, (), {}, None)

    def usage(self):
        """The account's usage against its quotas (`GET /auth/usage`, `usage`).

        The apps the account owns, its bundle bytes (a decimal string), API keys and owned organizations; the file bytes held by each payer it belongs to (each organization it is in, and `account:<id>` for its own apps); the cluster's quotas; and what today's runs of the apps the account develops used (decimal strings).

        **Who:** signed in (a session or an API key).

        Returns: UsageResponse200
        """
        return self._run(_USAGE, (), {}, None)


_CONSENT_PAGE_CREATE = Operation(
    _consent_page_create,
    "consent_page_create",
    body=(
        ConsentPageCreateBodyType0,
        ConsentPageCreateBodyType1,
        ConsentPageCreateBodyType10,
        ConsentPageCreateBodyType11,
        ConsentPageCreateBodyType12,
        ConsentPageCreateBodyType13,
        ConsentPageCreateBodyType14,
        ConsentPageCreateBodyType15,
        ConsentPageCreateBodyType2,
        ConsentPageCreateBodyType3,
        ConsentPageCreateBodyType4,
        ConsentPageCreateBodyType5,
        ConsentPageCreateBodyType6,
        ConsentPageCreateBodyType7,
        ConsentPageCreateBodyType8,
        ConsentPageCreateBodyType9,
    ),
    answer="json",
)
_CONSENT_PAGE_RESULT = Operation(
    _consent_page_result,
    "consent_page_result",
    body="none",
    answer="json",
)


class Consent(Group):
    """`client.consent`: the operations core's document groups as `consent`."""

    def page_create(self, *, body):
        """Ask the account's owner to approve (`POST /consent/requests`, `consent_page_create`).

        From an app's frontend: a release approval, an upgrade of its instance, outbound prefixes, a new organization, a new invite or joining with one, leaving an organization, a member's removal, role, teams or capabilities, an invite's revocation, an organization's name, its deletion, defaults or all-apps grants, approved on the node's own consent page (`url`), which takes the passkey. What the approval made (`orgId`, an invite's `token`, the `org` joined, a rename's `{ orgId, name }`, a deletion's `apps`) is in `/consent/{request}/result`'s `outcome`. 64 KiB at most.

        **Who:** a passkey session (an API key is refused).

        Args:
            body: the body, one of ConsentPageCreateBodyType0, ConsentPageCreateBodyType1, ConsentPageCreateBodyType10, ConsentPageCreateBodyType11, ConsentPageCreateBodyType12, ConsentPageCreateBodyType13, ConsentPageCreateBodyType14, ConsentPageCreateBodyType15, ConsentPageCreateBodyType2, ConsentPageCreateBodyType3, ConsentPageCreateBodyType4, ConsentPageCreateBodyType5, ConsentPageCreateBodyType6, ConsentPageCreateBodyType7, ConsentPageCreateBodyType8, ConsentPageCreateBodyType9

        Returns: ConsentPageCreateResponse200
        """
        return self._run(_CONSENT_PAGE_CREATE, (), {}, body)

    def page_result(self, *, request):
        """What became of a consent request (`GET /consent/{request}/result`, `consent_page_result`).

        `decidedAt` (Unix seconds) is `null` while it is pending. `outcome`, once approved, is what the approval made: `{ orgId }` for `createOrg` (with `teamId`, the asking app's own team the organization starts with, when an app's frontend asked on a cluster that makes it), `{ token }` for `createInvite`, `{ org }` for `acceptInvite`, `{ orgId, name }` for `renameOrg`, `{ apps }` (the apps deleted) for `deleteOrg`; absent for the other kinds.

        **Who:** a passkey session (an API key is refused).

        Args:
            request: path

        Returns: ConsentPageResultResponse200
        """
        return self._run(_CONSENT_PAGE_RESULT, (request,), {}, None)


_DEPLOYMENT_CREATE = Operation(
    _deployment_create,
    "deployment_create",
    body=DeploymentCreateBody,
    answer="json",
    confirm=True,
)
_DEPLOYMENT_DELETE = Operation(
    _deployment_delete,
    "deployment_delete",
    body=ConfirmOnly,
    answer="json",
    confirm=True,
)
_DEPLOYER_GRANT = Operation(
    _deployer_grant,
    "deployer_grant",
    body=DeployerGrantBody,
    answer="json",
    confirm=True,
)
_DEPLOYER_LIST = Operation(
    _deployer_list,
    "deployer_list",
    body="none",
    answer="json",
)
_DEPLOYER_REVOKE = Operation(
    _deployer_revoke, "deployer_revoke", body=ConfirmOnly, answer="json", confirm=True
)
_DEPLOYMENT_GET = Operation(
    _deployment_get,
    "deployment_get",
    body="none",
    answer="json",
)
_DEPLOYMENT_LIST = Operation(
    _deployment_list,
    "deployment_list",
    body="none",
    answer="json",
)
_DEPLOYMENT_OFFER = Operation(
    _deployment_offer,
    "deployment_offer",
    body=DeploymentOfferBody,
    answer="json",
    confirm=True,
)
_DEPLOYMENT_ORGS = Operation(
    _deployment_orgs,
    "deployment_orgs",
    body="none",
    answer="json",
)
_SIGN_UP_INVITE_CREATE = Operation(
    _sign_up_invite_create,
    "sign_up_invite_create",
    body=SignUpInviteCreateBody,
    answer="json",
    confirm=True,
)
_SIGN_UP_INVITE_LIST = Operation(
    _sign_up_invite_list,
    "sign_up_invite_list",
    body="none",
    answer="json",
)
_SIGN_UP_INVITE_REVOKE = Operation(
    _sign_up_invite_revoke,
    "sign_up_invite_revoke",
    body=ConfirmOnly,
    answer="json",
    confirm=True,
)
_SIGN_UP_PUBLIC = Operation(
    _sign_up_public,
    "sign_up_public",
    body="none",
    answer="json",
)
_SIGN_UP_SET = Operation(
    _sign_up_set, "sign_up_set", body=SignUpSetBody, answer="json", confirm=True
)
_DEPLOYMENT_WITHDRAW = Operation(
    _deployment_withdraw,
    "deployment_withdraw",
    body=ConfirmOnly,
    answer="json",
    confirm=True,
)


class Deployments(Group):
    """`client.deployments`: the operations core's document groups as `deployments`."""

    def create(self, *, name, owner, confirm=None):
        """Create a deployment (`POST /deployments`, `deployment_create`).

        A named set of releases an owner (an account or organization) offers for others to instantiate. Answers its id, the record as made (no apps yet) and its frontend host (`null` on a node with no apps domain).

        **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over `details.challenge`.

        Args:
            name: body `name`
            owner: body `owner`
            confirm: a passkey for the step-up, a ready `confirm`, or False

        Returns: DeploymentCreateResponse200
        """
        return self._run(
            _DEPLOYMENT_CREATE, (), {}, {"name": name, "owner": owner}, confirm=confirm
        )

    def delete(self, *, id, confirm=None):
        """Delete a deployment (`DELETE /deployments/{id}`, `deployment_delete`).

        **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over `details.challenge`.

        Args:
            id: path
            confirm: a passkey for the step-up, a ready `confirm`, or False

        Returns: DeploymentDeleteResponse200
        """
        return self._run(_DEPLOYMENT_DELETE, (id,), {}, {}, confirm=confirm)

    def deployer_grant(self, *, deployer, until=UNSET, confirm=None):
        """Grant a deployer (`POST /deployers`, `deployer_grant`).

        The cluster's owner, with a passkey.

        **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over `details.challenge`.

        Args:
            deployer: body `deployer`
            until: body `until`
            confirm: a passkey for the step-up, a ready `confirm`, or False

        Returns: DeployerGrantResponse200
        """
        return self._run(
            _DEPLOYER_GRANT,
            (),
            {},
            {"deployer": deployer, "until": until},
            confirm=confirm,
        )

    def deployer_list(self):
        """The deployers (`GET /deployers`, `deployer_list`).

        The operator, or the cluster's owner: the deployments and deployers governance, the owner (`null` until named), and every grant (`until` RFC 3339 and `untilSecs`, `null` for none; `grantedAt` RFC 3339; `deployer` `null` for a key it cannot read).

        **Who:** an account holder (a passkey session not bound to an app's frontend).

        Returns: DeployerListResponse200
        """
        return self._run(_DEPLOYER_LIST, (), {}, None)

    def deployer_revoke(self, *, key, confirm=None):
        """Revoke a deployer (`DELETE /deployers/{key}`, `deployer_revoke`).

        **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over `details.challenge`.

        Args:
            key: path
            confirm: a passkey for the step-up, a ready `confirm`, or False

        Returns: DeployerRevokeResponse200
        """
        return self._run(_DEPLOYER_REVOKE, (key,), {}, {}, confirm=confirm)

    def get(self, *, id):
        """One deployment (`GET /deployments/{id}`, `deployment_get`).

        As anyone may read it: its name, its owner's kind and name (`null` when unknown), and the releases it offers; never its managers or customers.

        **Who:** signed in (a session or an API key).

        Args:
            id: path

        Returns: DeploymentGetResponse200
        """
        return self._run(_DEPLOYMENT_GET, (id,), {}, None)

    def list(self):
        """Deployments (`GET /deployments`, `deployment_list`).

        The deployments the caller manages.

        **Who:** signed in (a session or an API key).

        Returns: DeploymentListResponse200
        """
        return self._run(_DEPLOYMENT_LIST, (), {}, None)

    def offer(self, *, id, app, package, version, confirm=None):
        """Offer a release in a deployment (`POST /deployments/{id}/apps`, `deployment_offer`).

        **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over `details.challenge`.

        Args:
            id: path
            app: body `app`
            package: body `package`
            version: body `version`
            confirm: a passkey for the step-up, a ready `confirm`, or False

        Returns: DeploymentOfferResponse200
        """
        return self._run(
            _DEPLOYMENT_OFFER,
            (id,),
            {},
            {"app": app, "package": package, "version": version},
            confirm=confirm,
        )

    def orgs(self, *, id):
        """The organizations a deployment serves (`GET /deployments/{id}/orgs`, `deployment_orgs`).

        For its managers: each organization with instances from it, its name (`null` when unknown), member counts (`activeMembers30d`: active in the last 30 days), its instances and the releases they run, and when its first was created (RFC 3339); instances outside an organization, or in a personal organization, are only counted (`outsideOrgs`). Never who the members are.

        **Who:** signed in (a session or an API key).

        Args:
            id: path

        Returns: DeploymentOrgsResponse200
        """
        return self._run(_DEPLOYMENT_ORGS, (id,), {}, None)

    def sign_up_invite_create(self, *, uses=UNSET, ttl_secs=UNSET, confirm=None):
        """Issue a cluster sign-up invite (`POST /sign-up/invites`, `sign_up_invite_create`).

        **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over `details.challenge`.

        Args:
            uses: body `uses`
            ttl_secs: body `ttlSecs`
            confirm: a passkey for the step-up, a ready `confirm`, or False

        Returns: SignUpInviteCreateResponse200
        """
        return self._run(
            _SIGN_UP_INVITE_CREATE,
            (),
            {},
            {"uses": uses, "ttlSecs": ttl_secs},
            confirm=confirm,
        )

    def sign_up_invite_list(self):
        """Cluster sign-up invites (`GET /sign-up/invites`, `sign_up_invite_list`).

        **Who:** an account holder (a passkey session not bound to an app's frontend).

        Returns: SignUpInviteListResponse200
        """
        return self._run(_SIGN_UP_INVITE_LIST, (), {}, None)

    def sign_up_invite_revoke(self, *, key, confirm=None):
        """Withdraw a cluster sign-up invite (`DELETE /sign-up/invites/{key}`, `sign_up_invite_revoke`).

        **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over `details.challenge`.

        Args:
            key: path
            confirm: a passkey for the step-up, a ready `confirm`, or False

        Returns: InviteKey
        """
        return self._run(_SIGN_UP_INVITE_REVOKE, (key,), {}, {}, confirm=confirm)

    def sign_up_public(self):
        """The sign-up policy (`GET /sign-up`, `sign_up_public`).

        Who may create an account here, the cluster's name and the tag a sign-in uses to pick the person's passkey for it.

        **Who:** anyone.

        Returns: SignUpPublicResponse200
        """
        return self._run(_SIGN_UP_PUBLIC, (), {}, None)

    def sign_up_set(self, *, policy, confirm=None):
        """Set the sign-up policy (`POST /sign-up`, `sign_up_set`).

        The cluster's owner, once named, with a passkey (`412` first).

        **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over `details.challenge`.

        Args:
            policy: body `policy`
            confirm: a passkey for the step-up, a ready `confirm`, or False

        Returns: SignUpSet
        """
        return self._run(_SIGN_UP_SET, (), {}, {"policy": policy}, confirm=confirm)

    def withdraw(self, *, id, app, confirm=None):
        """Withdraw an offer (`DELETE /deployments/{id}/apps/{app}`, `deployment_withdraw`).

        **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over `details.challenge`.

        Args:
            id: path
            app: path
            confirm: a passkey for the step-up, a ready `confirm`, or False

        Returns: DeploymentWithdrawResponse200
        """
        return self._run(
            _DEPLOYMENT_WITHDRAW,
            (
                id,
                app,
            ),
            {},
            {},
            confirm=confirm,
        )


_OPS_APPS = Operation(
    _ops_apps,
    "ops_apps",
    body="none",
    answer="json",
)
_OPS_BACKUP = Operation(
    _ops_backup,
    "ops_backup",
    body="none",
    answer="json",
)
_OPS_BACKUP_KEY = Operation(
    _ops_backup_key,
    "ops_backup_key",
    body="none",
    answer="json",
)
_OPS_CLUSTER = Operation(
    _ops_cluster,
    "ops_cluster",
    body="none",
    answer="json",
)
_OPS_CLUSTER_OWNER = Operation(
    _ops_cluster_owner,
    "ops_cluster_owner",
    body=OpsClusterOwnerBody,
    answer="json",
    operator=True,
)
_OPS_DEPLOYER_GRANT = Operation(
    _ops_deployer_grant,
    "ops_deployer_grant",
    body=OpsDeployerGrantBody,
    answer="json",
    operator=True,
)
_OPS_DEPLOYER_REVOKE_DELETE = Operation(
    _ops_deployer_revoke_delete,
    "ops_deployer_revoke_delete",
    body=ProofOnly,
    answer="json",
    operator=True,
)
_OPS_DEPLOYER_REVOKE_POST = Operation(
    _ops_deployer_revoke_post,
    "ops_deployer_revoke_post",
    body=ProofOnly,
    answer="json",
    operator=True,
)
_OPS_DEPLOYMENTS_GRANTED = Operation(
    _ops_deployments_granted,
    "ops_deployments_granted",
    body=ProofOnly,
    answer="json",
    operator=True,
)
_OPS_JOB_NODE_ATTESTATION = Operation(
    _ops_job_node_attestation,
    "ops_job_node_attestation",
    body="none",
    answer="json",
)
_OPS_METRICS = Operation(
    _ops_metrics,
    "ops_metrics",
    body="none",
    answer="text",
)
_OPS_ORG_UPGRADE = Operation(
    _ops_org_upgrade,
    "ops_org_upgrade",
    body=OpsOrgUpgradeBody,
    answer="json",
    operator=True,
)
_OPS_ORGS = Operation(
    _ops_orgs,
    "ops_orgs",
    body="none",
    answer="json",
)
_OPS_OUTBOUND = Operation(
    _ops_outbound, "ops_outbound", body=OpsOutboundBody, answer="json", operator=True
)
_OPS_PACKAGE_INSTALL = Operation(
    _ops_package_install,
    "ops_package_install",
    body=OpsPackageInstallBody,
    answer="json",
)
_OPS_PACKAGE_LIST = Operation(
    _ops_package_list,
    "ops_package_list",
    body="none",
    answer="json",
)
_OPS_PACKAGE_UNINSTALL = Operation(
    _ops_package_uninstall,
    "ops_package_uninstall",
    body="none",
    answer="json",
)
_OPS_PACKAGE_UPLOAD = Operation(
    _ops_package_upload,
    "ops_package_upload",
    body="bytes",
    answer="json",
)
_OPS_PACKAGE_VERSION_UNINSTALL = Operation(
    _ops_package_version_uninstall,
    "ops_package_version_uninstall",
    body="none",
    answer="json",
)
_OPS_PLACEMENT = Operation(
    _ops_placement,
    "ops_placement",
    body="none",
    answer="json",
)
_OPS_POOLS_GET = Operation(
    _ops_pools_get,
    "ops_pools_get",
    body="none",
    answer="json",
)
_OPS_POOLS_SET = Operation(
    _ops_pools_set, "ops_pools_set", body=OpsPoolsSetBody, answer="json", operator=True
)
_OPS_PRICES_GET = Operation(
    _ops_prices_get,
    "ops_prices_get",
    body="none",
    answer="json",
)
_OPS_PRICES_SET = Operation(
    _ops_prices_set,
    "ops_prices_set",
    body=OpsPricesSetBody,
    answer="json",
    operator=True,
)
_OPS_QUOTAS_GET = Operation(
    _ops_quotas_get,
    "ops_quotas_get",
    body="none",
    answer="json",
)
_OPS_QUOTAS_SET = Operation(
    _ops_quotas_set, "ops_quotas_set", body=QuotaChanges, answer="json", operator=True
)
_OPS_RECEIPT_KEY = Operation(
    _ops_receipt_key,
    "ops_receipt_key",
    body="none",
    answer="json",
)
_OPS_RECOVER = Operation(
    _ops_recover,
    "ops_recover",
    body=OpsRecoverBody,
    answer="json",
)
_OPS_RECOVERY_FILES = Operation(
    _ops_recovery_files,
    "ops_recovery_files",
    body="none",
    answer="json",
)
_OPS_REGISTER_NODE = Operation(
    _ops_register_node,
    "ops_register_node",
    body=OpsRegisterNodeBody,
    answer="json",
    operator=True,
)
_OPS_RELEASE = Operation(
    _ops_release, "ops_release", body=OpsReleaseBody, answer="json", operator=True
)
_OPS_RELEASE_DEADLINE = Operation(
    _ops_release_deadline,
    "ops_release_deadline",
    body=OpsReleaseDeadlineBody,
    answer="json",
    operator=True,
)
_OPS_RELEASE_DEADLINE_LIFT = Operation(
    _ops_release_deadline_lift,
    "ops_release_deadline_lift",
    body=ProofOnly,
    answer="json",
    operator=True,
)
_OPS_REMOVE_NODE = Operation(
    _ops_remove_node,
    "ops_remove_node",
    body=ProofOnly,
    answer="json",
    operator=True,
    leader=True,
)
_OPS_RESTORE = Operation(
    _ops_restore, "ops_restore", body=OpsRestoreBody, answer="json", operator=True
)
_OPS_SERVED = Operation(
    _ops_served,
    "ops_served",
    body="none",
    answer="json",
)
_OPS_SET_BACKUP_KEY = Operation(
    _ops_set_backup_key,
    "ops_set_backup_key",
    body=OpsSetBackupKeyBody,
    answer="json",
    operator=True,
)
_OPS_SET_REPLICAS = Operation(
    _ops_set_replicas,
    "ops_set_replicas",
    body=OpsSetReplicasBody,
    answer="json",
    operator=True,
    leader=True,
)
_OPS_SIGN_UP_INVITE_CREATE = Operation(
    _ops_sign_up_invite_create,
    "ops_sign_up_invite_create",
    body=OpsSignUpInviteCreateBody,
    answer="json",
    operator=True,
)
_OPS_SIGN_UP_INVITE_LIST = Operation(
    _ops_sign_up_invite_list,
    "ops_sign_up_invite_list",
    body="none",
    answer="json",
)
_OPS_SIGN_UP_INVITE_REVOKE_DELETE = Operation(
    _ops_sign_up_invite_revoke_delete,
    "ops_sign_up_invite_revoke_delete",
    body=ProofOnly,
    answer="json",
    operator=True,
)
_OPS_SIGN_UP_INVITE_REVOKE_POST = Operation(
    _ops_sign_up_invite_revoke_post,
    "ops_sign_up_invite_revoke_post",
    body=ProofOnly,
    answer="json",
    operator=True,
)
_OPS_SIGN_UP_SET = Operation(
    _ops_sign_up_set,
    "ops_sign_up_set",
    body=OpsSignUpSetBody,
    answer="json",
    operator=True,
)
_OPS_STATUS = Operation(
    _ops_status,
    "ops_status",
    body="none",
    answer="json",
)
_OPS_UPGRADE = Operation(
    _ops_upgrade, "ops_upgrade", body=OpsUpgradeBody, answer="json", operator=True
)


class Ops(Group):
    """`client.ops`: the operations core's document groups as `ops`."""

    def apps(self, *, after=UNSET, limit=UNSET):
        """What each app runs (`GET /ops/apps`, `ops_apps`).

        Each app's bundle, package, publisher, who holds its upgrades and release approvals, its last upgrade, approved releases, replicas' releases and `lastWriteAt`.

        **Who:** the ops token.

        Args:
            after: query
            limit: query

        Returns: OpsAppsResponse200
        """
        return self._run(_OPS_APPS, (), {"after": after, "limit": limit}, None)

    def backup(self, *, all_=UNSET):
        """Snapshot the apps this node leads (`POST /ops/backup`, `ops_backup`).

        Into `--backup-dir`. With `all=true`, every group this node holds, leader or not: what a node of a cluster that lost a majority still has, to found the cluster again from (`merod --bootstrap --restore-from`). `written`: how many snapshots it wrote.

        **Who:** the ops token.

        Args:
            all_: query

        Returns: OpsBackupResponse200
        """
        return self._run(_OPS_BACKUP, (), {"all_": all_}, None)

    def backup_key(self):
        """The operator's backup key (`GET /ops/backup-key`, `ops_backup_key`).

        The X25519 public key every app's recovery-disk backups are sealed to where the operator upgrades apps (`key`, `null` while none is set).

        **Who:** the ops token.

        Returns: BackupKeyAnswer
        """
        return self._run(_OPS_BACKUP_KEY, (), {}, None)

    def cluster(self):
        """The cluster's identity (`GET /ops/cluster`, `ops_cluster`).

        The cluster's name and the identity this replica holds (hex).

        **Who:** the ops token.

        Returns: OpsClusterResponse200
        """
        return self._run(_OPS_CLUSTER, (), {}, None)

    def cluster_owner(self, *, owner, operator_proof=None):
        """Name the cluster's owner (`POST /ops/cluster-owner`, `ops_cluster_owner`).

        The account or organization that then runs sign-up and deployers with passkeys; once.

        **Who:** the ops token. **Operator signature:** answers `428 OPERATOR_SIGNATURE_REQUIRED` first; send it again with `operatorProof`, the operator key's signature over `details.message` and its `nonce`.

        Args:
            owner: body `owner`
            operator_proof: an operator seed (hex), a signer, a ready `operatorProof`, or False

        Returns: OpsClusterOwnerResponse200
        """
        return self._run(
            _OPS_CLUSTER_OWNER, (), {}, {"owner": owner}, operator_proof=operator_proof
        )

    def deployer_grant(self, *, deployer, until=UNSET, operator_proof=None):
        """Grant a deployer (`POST /ops/deployers`, `ops_deployer_grant`).

        An account or organization that may install where deployments are granted, `until` an RFC 3339 time.

        **Who:** the ops token. **Operator signature:** answers `428 OPERATOR_SIGNATURE_REQUIRED` first; send it again with `operatorProof`, the operator key's signature over `details.message` and its `nonce`.

        Args:
            deployer: body `deployer`
            until: body `until`
            operator_proof: an operator seed (hex), a signer, a ready `operatorProof`, or False

        Returns: OpsDeployerGrantResponse200
        """
        return self._run(
            _OPS_DEPLOYER_GRANT,
            (),
            {},
            {"deployer": deployer, "until": until},
            operator_proof=operator_proof,
        )

    def deployer_revoke_delete(self, *, key, operator_proof=None):
        """Revoke a deployer (`DELETE /ops/deployers/{key}`, `ops_deployer_revoke_delete`).

        **Who:** the ops token. **Operator signature:** answers `428 OPERATOR_SIGNATURE_REQUIRED` first; send it again with `operatorProof`, the operator key's signature over `details.message` and its `nonce`.

        Args:
            key: path
            operator_proof: an operator seed (hex), a signer, a ready `operatorProof`, or False

        Returns: OpsDeployerRevokeDeleteResponse200
        """
        return self._run(
            _OPS_DEPLOYER_REVOKE_DELETE, (key,), {}, {}, operator_proof=operator_proof
        )

    def deployer_revoke_post(self, *, key, operator_proof=None):
        """Revoke a deployer (POST) (`POST /ops/deployers/{key}/revoke`, `ops_deployer_revoke_post`).

        The same as the `DELETE`, for an HTTP client whose `DELETE` carries no body.

        **Who:** the ops token. **Operator signature:** answers `428 OPERATOR_SIGNATURE_REQUIRED` first; send it again with `operatorProof`, the operator key's signature over `details.message` and its `nonce`.

        Args:
            key: path
            operator_proof: an operator seed (hex), a signer, a ready `operatorProof`, or False

        Returns: OpsDeployerRevokePostResponse200
        """
        return self._run(
            _OPS_DEPLOYER_REVOKE_POST, (key,), {}, {}, operator_proof=operator_proof
        )

    def deployments_granted(self, *, operator_proof=None):
        """Require deployer grants (`POST /ops/deployments/granted`, `ops_deployments_granted`).

        From now on only granted deployers install; once.

        **Who:** the ops token. **Operator signature:** answers `428 OPERATOR_SIGNATURE_REQUIRED` first; send it again with `operatorProof`, the operator key's signature over `details.message` and its `nonce`.

        Args:
            operator_proof: an operator seed (hex), a signer, a ready `operatorProof`, or False

        Returns: OpsDeploymentsGrantedResponse200
        """
        return self._run(
            _OPS_DEPLOYMENTS_GRANTED, (), {}, {}, operator_proof=operator_proof
        )

    def job_node_attestation(self, *, id):
        """Whether the cluster accepts a job node (`GET /ops/job-nodes/{id}/attestation`, `ops_job_node_attestation`).

        Runs the check the leader makes before it sends a job node work: a quote that passes the release policy, this cluster and node, this cluster's identity, a release not retired (not the per-app release approval). `admitted` carries the attested `release`; `refused` (final), with the reason: a 400 or 404 from the node, a 2xx that is oversized or does not decode, a failed verification, or a 400 or 404 from the collateral source on the PCK certificate or TCB info request; `unreachable` (transient), with the reason: anything else, including no answer, any other status from the node and any other collateral failure. `503` when this node's directory is stale. A verified answer is reused for up to a minute.

        **Who:** the ops token.

        Args:
            id: path

        Returns: ForwardRef('OpsJobNodeAttestationResponse200Type0') | ForwardRef('OpsJobNodeAttestationResponse200Type1')
        """
        return self._run(_OPS_JOB_NODE_ATTESTATION, (id,), {}, None)

    def metrics(self):
        """Prometheus metrics (`GET /ops/metrics`, `ops_metrics`).

        Including per-app usage for billing. `--metrics-listen ADDR` also serves it as plain `GET /metrics` there.

        **Who:** the ops token.

        Returns: str
        """
        return self._run(_OPS_METRICS, (), {}, None)

    def org_upgrade(
        self,
        *,
        org,
        package,
        version,
        team=UNSET,
        cascade=UNSET,
        migrate=UNSET,
        operator_proof=None,
    ):
        """An operator's upgrade run (`POST /ops/orgs/{org}/upgrade`, `ops_org_upgrade`).

        **Who:** the ops token. **Operator signature:** answers `428 OPERATOR_SIGNATURE_REQUIRED` first; send it again with `operatorProof`, the operator key's signature over `details.message` and its `nonce`.

        Args:
            org: path
            package: body `package`
            version: body `version`
            team: body `team`
            cascade: body `cascade`
            migrate: body `migrate`
            operator_proof: an operator seed (hex), a signer, a ready `operatorProof`, or False

        Returns: UpgradeRun
        """
        return self._run(
            _OPS_ORG_UPGRADE,
            (org,),
            {},
            {
                "package": package,
                "version": version,
                "team": team,
                "cascade": cascade,
                "migrate": migrate,
            },
            operator_proof=operator_proof,
        )

    def orgs(self, *, after=UNSET, limit=UNSET):
        """Each organization's approved releases (`GET /ops/orgs`, `ops_orgs`).

        **Who:** the ops token.

        Args:
            after: query
            limit: query

        Returns: OpsOrgsResponse200
        """
        return self._run(_OPS_ORGS, (), {"after": after, "limit": limit}, None)

    def outbound(self, *, package, prefixes=UNSET, replace=UNSET, operator_proof=None):
        """Approve outbound prefixes (`POST /ops/outbound`, `ops_outbound`).

        Where the operator holds outbound approvals.

        **Who:** the ops token. **Operator signature:** answers `428 OPERATOR_SIGNATURE_REQUIRED` first; send it again with `operatorProof`, the operator key's signature over `details.message` and its `nonce`.

        Args:
            package: body `package`
            prefixes: body `prefixes`
            replace: body `replace`
            operator_proof: an operator seed (hex), a signer, a ready `operatorProof`, or False

        Returns: OutboundApproved
        """
        return self._run(
            _OPS_OUTBOUND,
            (),
            {},
            {"package": package, "prefixes": prefixes, "replace": replace},
            operator_proof=operator_proof,
        )

    def package_install(self, *, package, version):
        """Install a release from the registry as the operator (`POST /ops/packages/install`, `ops_package_install`).

        **Who:** the ops token.

        Args:
            package: body `package`
            version: body `version`

        Returns: OpsPackageInstallResponse200
        """
        return self._run(
            _OPS_PACKAGE_INSTALL, (), {}, {"package": package, "version": version}
        )

    def package_list(self):
        """The releases the operator installed (`GET /ops/packages`, `ops_package_list`).

        **Who:** the ops token.

        Returns: OpsPackageListResponse200
        """
        return self._run(_OPS_PACKAGE_LIST, (), {}, None)

    def package_uninstall(self, *, package):
        """Uninstall what the operator installed (`DELETE /ops/packages/{package}`, `ops_package_uninstall`).

        **Who:** the ops token.

        Args:
            package: path

        Returns: Uninstalled
        """
        return self._run(_OPS_PACKAGE_UNINSTALL, (package,), {}, None)

    def package_upload(self, *, body):
        """Install an uploaded release as the operator (`POST /ops/packages`, `ops_package_upload`).

        **Who:** the ops token.

        Args:
            body: the raw bytes

        Returns: OpsPackageUploadResponse200
        """
        return self._run(_OPS_PACKAGE_UPLOAD, (), {}, body)

    def package_version_uninstall(self, *, package, version):
        """Uninstall one operator-installed version (`DELETE /ops/packages/{package}/{version}`, `ops_package_version_uninstall`).

        **Who:** the ops token.

        Args:
            package: path
            version: path

        Returns: Uninstalled
        """
        return self._run(
            _OPS_PACKAGE_VERSION_UNINSTALL,
            (
                package,
                version,
            ),
            {},
            None,
        )

    def placement(self):
        """Where every app's replicas are (`GET /ops/placement`, `ops_placement`).

        Ids and counts only, read after a read index (`fresh`: this node caught up with the system leader); `release` is a node's attested release; `clusterVersion` is the logged cluster version (`null` until the system leader logs one).

        **Who:** the ops token.

        Returns: OpsPlacementResponse200
        """
        return self._run(_OPS_PLACEMENT, (), {}, None)

    def pools_get(self):
        """Compute pools (`GET /ops/pools`, `ops_pools_get`).

        The pools by name (absent: none set, one legacy pool), and each pool's capacity: how many runs its job nodes take at once.

        **Who:** the ops token.

        Returns: OpsPoolsGetResponse200
        """
        return self._run(_OPS_POOLS_GET, (), {}, None)

    def pools_set(self, *, pools, operator_proof=None):
        """Set the compute pools (`POST /ops/pools`, `ops_pools_set`).

        Signed over `cluster / __set_pools / { pools }`; `POOL_IN_USE` when a pool to remove still has job nodes or runs.

        **Who:** the ops token. **Operator signature:** answers `428 OPERATOR_SIGNATURE_REQUIRED` first; send it again with `operatorProof`, the operator key's signature over `details.message` and its `nonce`.

        Args:
            pools: body `pools`
            operator_proof: an operator seed (hex), a signer, a ready `operatorProof`, or False

        Returns: OpsPoolsSetResponse200
        """
        return self._run(
            _OPS_POOLS_SET, (), {}, {"pools": pools}, operator_proof=operator_proof
        )

    def prices_get(self):
        """The price table (`GET /ops/prices`, `ops_prices_get`).

        In micro-units of the billing currency; `{}` until one is set.

        **Who:** the ops token.

        Returns: OpsPricesGetResponse200
        """
        return self._run(_OPS_PRICES_GET, (), {}, None)

    def prices_set(self, *, table, operator_proof=None):
        """Set the price table (`POST /ops/prices`, `ops_prices_set`).

        Signed over `cluster / __set_price_table / { table }`; `PRICE_VERSION` unless `version` is the current one plus one (1 for the first), `BAD_PRICES` for a zero `fuelPerVcpuSecond`.

        **Who:** the ops token. **Operator signature:** answers `428 OPERATOR_SIGNATURE_REQUIRED` first; send it again with `operatorProof`, the operator key's signature over `details.message` and its `nonce`.

        Args:
            table: body `table`
            operator_proof: an operator seed (hex), a signer, a ready `operatorProof`, or False

        Returns: OpsPricesSetResponse200
        """
        return self._run(
            _OPS_PRICES_SET, (), {}, {"table": table}, operator_proof=operator_proof
        )

    def quotas_get(self):
        """The quotas (`GET /ops/quotas`, `ops_quotas_get`).

        The per-account quotas, and the per-payer overrides under `orgs`.

        **Who:** the ops token.

        Returns: QuotaSettings
        """
        return self._run(_OPS_QUOTAS_GET, (), {}, None)

    def quotas_set(
        self,
        *,
        org=UNSET,
        orgs=UNSET,
        max_apps_per_account=UNSET,
        max_bundle_bytes_per_account=UNSET,
        max_api_keys_per_account=UNSET,
        max_orgs_per_account=UNSET,
        max_accounts=UNSET,
        max_file_bytes_per_org=UNSET,
        max_concurrent_runs_per_org=UNSET,
        compute_fuel_per_org=UNSET,
        max_fuel_per_developer_per_day=UNSET,
        max_concurrent_runs=UNSET,
        compute_fuel=UNSET,
        max_file_bytes=UNSET,
        operator_proof=None,
    ):
        """Change quotas (`POST /ops/quotas`, `ops_quotas_set`).

        Changes the limits the body names, the others keep their value; signed over `cluster / __set_quotas / { quotas }` with every limit after the change. Without `org`, the cluster's limits (`GET /ops/quotas` names them); `orgs`, which that answer carries, is ignored, so its answer may be sent back changed. With `org`: one payer's `maxConcurrentRuns`, `computeFuel` (decimal string) and `maxFileBytes` (`null` clears one, back to the cluster's), and no other limit. A property that is no limit (of the cluster's, or with `org` of a payer's) is refused `400 UNKNOWN_QUOTA`.

        **Who:** the ops token. **Operator signature:** answers `428 OPERATOR_SIGNATURE_REQUIRED` first; send it again with `operatorProof`, the operator key's signature over `details.message` and its `nonce`.

        Args:
            org: body `org`
            orgs: body `orgs`
            max_apps_per_account: body `maxAppsPerAccount`
            max_bundle_bytes_per_account: body `maxBundleBytesPerAccount`
            max_api_keys_per_account: body `maxApiKeysPerAccount`
            max_orgs_per_account: body `maxOrgsPerAccount`
            max_accounts: body `maxAccounts`
            max_file_bytes_per_org: body `maxFileBytesPerOrg`
            max_concurrent_runs_per_org: body `maxConcurrentRunsPerOrg`
            compute_fuel_per_org: body `computeFuelPerOrg`
            max_fuel_per_developer_per_day: body `maxFuelPerDeveloperPerDay`
            max_concurrent_runs: body `maxConcurrentRuns`
            compute_fuel: body `computeFuel`
            max_file_bytes: body `maxFileBytes`
            operator_proof: an operator seed (hex), a signer, a ready `operatorProof`, or False

        Returns: OpsQuotasSetResponse200
        """
        return self._run(
            _OPS_QUOTAS_SET,
            (),
            {},
            {
                "org": org,
                "orgs": orgs,
                "maxAppsPerAccount": max_apps_per_account,
                "maxBundleBytesPerAccount": max_bundle_bytes_per_account,
                "maxApiKeysPerAccount": max_api_keys_per_account,
                "maxOrgsPerAccount": max_orgs_per_account,
                "maxAccounts": max_accounts,
                "maxFileBytesPerOrg": max_file_bytes_per_org,
                "maxConcurrentRunsPerOrg": max_concurrent_runs_per_org,
                "computeFuelPerOrg": compute_fuel_per_org,
                "maxFuelPerDeveloperPerDay": max_fuel_per_developer_per_day,
                "maxConcurrentRuns": max_concurrent_runs,
                "computeFuel": compute_fuel,
                "maxFileBytes": max_file_bytes,
            },
            operator_proof=operator_proof,
        )

    def receipt_key(self, *, id):
        """A node's receipt key (`GET /ops/nodes/{id}/receipt-key`, `ops_receipt_key`).

        **Who:** the ops token.

        Args:
            id: path

        Returns: OpsReceiptKeyResponse200
        """
        return self._run(_OPS_RECEIPT_KEY, (id,), {}, None)

    def recover(self, *, system, code=UNSET):
        """Found the cluster again from its recovery disk (`POST /ops/recover`, `ops_recover`).

        On the founder started with `--bootstrap --recover-from DIR`, its system state empty: the newest `system` file in `DIR/operator/` and the apps' `code` files, each with the data key the operator opened from it (hex). Checked as founding from backups is (governance, operator key, no node of the old cluster answering), then the system group is seeded; each app waits for `POST /ops/apps/{app}/restore { recovery }`.

        **Who:** the ops token.

        Args:
            system: body `system`
            code: body `code`

        Returns: OpsRecoverResponse200
        """
        return self._run(_OPS_RECOVER, (), {}, {"system": system, "code": code})

    def recovery_files(self, *, holder=UNSET):
        """The recovery files a node reads (`GET /ops/recovery-files`, `ops_recovery_files`).

        In `--recover-from`, else `--recovery-dir`, of `holder` (`operator`, the default; `org:<id>`; `account:<id>`): each file's name, its header as signed (JSON text) and the node's Ed25519 signature over `calimero.recovery-backup.v1\\0` and those bytes (hex), for the holder's device to check before it opens a key with its own.

        **Who:** the ops token.

        Args:
            holder: query

        Returns: OpsRecoveryFilesResponse200
        """
        return self._run(_OPS_RECOVERY_FILES, (), {"holder": holder}, None)

    def register_node(
        self, *, id, addr, zone=UNSET, role=UNSET, pool=UNSET, operator_proof=None
    ):
        """Register a node (`POST /ops/nodes`, `ops_register_node`).

        Signed with the operator key over `cluster / __register_node / { id, addr, zone }`. The system leader adds it as a learner and promotes caught-up learners while fewer than 3 voters are left; `role: job` registers a compute node (in `pool`).

        **Who:** the ops token. **Operator signature:** answers `428 OPERATOR_SIGNATURE_REQUIRED` first; send it again with `operatorProof`, the operator key's signature over `details.message` and its `nonce`.

        Args:
            id: body `id`
            addr: body `addr`
            zone: body `zone`
            role: body `role`
            pool: body `pool`
            operator_proof: an operator seed (hex), a signer, a ready `operatorProof`, or False

        Returns: OpsRegisterNodeResponse200
        """
        return self._run(
            _OPS_REGISTER_NODE,
            (),
            {},
            {"id": id, "addr": addr, "zone": zone, "role": role, "pool": pool},
            operator_proof=operator_proof,
        )

    def release(self, *, version, approved=UNSET, operator_proof=None):
        """Approve a mero-tee release for every app (`POST /ops/releases`, `ops_release`).

        Where the operator holds release approvals.

        **Who:** the ops token. **Operator signature:** answers `428 OPERATOR_SIGNATURE_REQUIRED` first; send it again with `operatorProof`, the operator key's signature over `details.message` and its `nonce`.

        Args:
            version: body `version`
            approved: body `approved`
            operator_proof: an operator seed (hex), a signer, a ready `operatorProof`, or False

        Returns: ApprovedReleases
        """
        return self._run(
            _OPS_RELEASE,
            (),
            {},
            {"version": version, "approved": approved},
            operator_proof=operator_proof,
        )

    def release_deadline(self, *, version, after=UNSET, operator_proof=None):
        """Set a release deadline (`POST /ops/releases/{version}/deadline`, `ops_release_deadline`).

        After `after` (RFC 3339), apps whose approver has not moved past the release may start no new work.

        **Who:** the ops token. **Operator signature:** answers `428 OPERATOR_SIGNATURE_REQUIRED` first; send it again with `operatorProof`, the operator key's signature over `details.message` and its `nonce`.

        Args:
            version: path
            after: body `after`
            operator_proof: an operator seed (hex), a signer, a ready `operatorProof`, or False

        Returns: Deadlines
        """
        return self._run(
            _OPS_RELEASE_DEADLINE,
            (version,),
            {},
            {"after": after},
            operator_proof=operator_proof,
        )

    def release_deadline_lift(self, *, version, operator_proof=None):
        """Lift a release deadline (`DELETE /ops/releases/{version}/deadline`, `ops_release_deadline_lift`).

        **Who:** the ops token. **Operator signature:** answers `428 OPERATOR_SIGNATURE_REQUIRED` first; send it again with `operatorProof`, the operator key's signature over `details.message` and its `nonce`.

        Args:
            version: path
            operator_proof: an operator seed (hex), a signer, a ready `operatorProof`, or False

        Returns: Deadlines
        """
        return self._run(
            _OPS_RELEASE_DEADLINE_LIFT,
            (version,),
            {},
            {},
            operator_proof=operator_proof,
        )

    def remove_node(self, *, id, operator_proof=None):
        """Remove a node (`DELETE /ops/nodes/{id}`, `ops_remove_node`).

        One no app keeps a replica on; the system leader (elsewhere `421 NOT_LEADER`); signed over `__remove_node / { id }`.

        **Who:** the ops token. **Operator signature:** answers `428 OPERATOR_SIGNATURE_REQUIRED` first; send it again with `operatorProof`, the operator key's signature over `details.message` and its `nonce`.

        Args:
            id: path
            operator_proof: an operator seed (hex), a signer, a ready `operatorProof`, or False

        Returns: OpsRemoveNodeResponse200
        """
        return self._run(_OPS_REMOVE_NODE, (id,), {}, {}, operator_proof=operator_proof)

    def restore(self, *, app, backup=UNSET, recovery=UNSET, operator_proof=None):
        """Restore an app from its sealed backup (`POST /ops/apps/{app}/restore`, `ops_restore`).

        Signed over `<app> / __restore_app / { backup, sha256 }`. The receiving node holds the backup (`backup`, else the newest, in `{backup-dir}/{app}/`) and seeds the restore; answers once every replica holds the restored state: the rows restored, the backup (`format` 0, with `merod` and `release` `null`, for one written before manifests) and the app group's Raft status. `recovery` restores from a recovery-disk file instead (in the app's holder's directory under `--recover-from`, else `--recovery-dir`), opened with the data key its holder opened from it (hex); `backup` is then the file's name and `sha256` its body's.

        **Who:** the ops token. **Operator signature:** answers `428 OPERATOR_SIGNATURE_REQUIRED` first; send it again with `operatorProof`, the operator key's signature over `details.message` and its `nonce`.

        Args:
            app: path
            backup: body `backup`
            recovery: body `recovery`
            operator_proof: an operator seed (hex), a signer, a ready `operatorProof`, or False

        Returns: OpsRestoreResponse200
        """
        return self._run(
            _OPS_RESTORE,
            (app,),
            {},
            {"backup": backup, "recovery": recovery},
            operator_proof=operator_proof,
        )

    def served(self):
        """Custom domains to serve (`GET /ops/served-origins`, `ops_served`).

        Every served origin a package's manager registered, by the package's label, never its name; `addedAt` is RFC 3339.

        **Who:** the ops token.

        Returns: OpsServedResponse200
        """
        return self._run(_OPS_SERVED, (), {}, None)

    def set_backup_key(self, *, public_key, operator_proof=None):
        """Set the operator's backup key (`POST /ops/backup-key`, `ops_set_backup_key`).

        Signed over `operator / __set_backup_key / { publicKey }`, where the operator upgrades apps (else `403`). `publicKey`: 64 lowercase hex, an X25519 public key whose private half the operator keeps (never a node).

        **Who:** the ops token. **Operator signature:** answers `428 OPERATOR_SIGNATURE_REQUIRED` first; send it again with `operatorProof`, the operator key's signature over `details.message` and its `nonce`.

        Args:
            public_key: body `publicKey`
            operator_proof: an operator seed (hex), a signer, a ready `operatorProof`, or False

        Returns: BackupKeyAnswer
        """
        return self._run(
            _OPS_SET_BACKUP_KEY,
            (),
            {},
            {"publicKey": public_key},
            operator_proof=operator_proof,
        )

    def set_replicas(self, *, app, replicas, operator_proof=None):
        """Move an app's replicas (`POST /ops/apps/{app}/replicas`, `ops_set_replicas`).

        Signed over `<app> / __set_replicas / { replicas }` (sorted); at least `min(3, nodes)` registered nodes, each on a release the app's owner approved. The set it already has answers `unchanged: true`. `status` is the app group's Raft status when the move finished (as the leader saw it).

        **Who:** the ops token. **Operator signature:** answers `428 OPERATOR_SIGNATURE_REQUIRED` first; send it again with `operatorProof`, the operator key's signature over `details.message` and its `nonce`.

        Args:
            app: path
            replicas: body `replicas`
            operator_proof: an operator seed (hex), a signer, a ready `operatorProof`, or False

        Returns: OpsSetReplicasResponse200
        """
        return self._run(
            _OPS_SET_REPLICAS,
            (app,),
            {},
            {"replicas": replicas},
            operator_proof=operator_proof,
        )

    def sign_up_invite_create(
        self, *, token, uses=UNSET, expires_at, operator_proof=None
    ):
        """Issue a cluster sign-up invite (`POST /ops/sign-up/invites`, `ops_sign_up_invite_create`).

        **Who:** the ops token. **Operator signature:** answers `428 OPERATOR_SIGNATURE_REQUIRED` first; send it again with `operatorProof`, the operator key's signature over `details.message` and its `nonce`.

        Args:
            token: body `token`
            uses: body `uses`
            expires_at: body `expiresAt`
            operator_proof: an operator seed (hex), a signer, a ready `operatorProof`, or False

        Returns: SignUpInvite
        """
        return self._run(
            _OPS_SIGN_UP_INVITE_CREATE,
            (),
            {},
            {"token": token, "uses": uses, "expiresAt": expires_at},
            operator_proof=operator_proof,
        )

    def sign_up_invite_list(self):
        """Cluster sign-up invites (`GET /ops/sign-up/invites`, `ops_sign_up_invite_list`).

        **Who:** the ops token.

        Returns: OpsSignUpInviteListResponse200
        """
        return self._run(_OPS_SIGN_UP_INVITE_LIST, (), {}, None)

    def sign_up_invite_revoke_delete(self, *, key, operator_proof=None):
        """Withdraw a cluster sign-up invite (`DELETE /ops/sign-up/invites/{key}`, `ops_sign_up_invite_revoke_delete`).

        **Who:** the ops token. **Operator signature:** answers `428 OPERATOR_SIGNATURE_REQUIRED` first; send it again with `operatorProof`, the operator key's signature over `details.message` and its `nonce`.

        Args:
            key: path
            operator_proof: an operator seed (hex), a signer, a ready `operatorProof`, or False

        Returns: InviteKey
        """
        return self._run(
            _OPS_SIGN_UP_INVITE_REVOKE_DELETE,
            (key,),
            {},
            {},
            operator_proof=operator_proof,
        )

    def sign_up_invite_revoke_post(self, *, key, operator_proof=None):
        """Withdraw a cluster sign-up invite (POST) (`POST /ops/sign-up/invites/{key}/revoke`, `ops_sign_up_invite_revoke_post`).

        For an HTTP client whose `DELETE` carries no body.

        **Who:** the ops token. **Operator signature:** answers `428 OPERATOR_SIGNATURE_REQUIRED` first; send it again with `operatorProof`, the operator key's signature over `details.message` and its `nonce`.

        Args:
            key: path
            operator_proof: an operator seed (hex), a signer, a ready `operatorProof`, or False

        Returns: InviteKey
        """
        return self._run(
            _OPS_SIGN_UP_INVITE_REVOKE_POST,
            (key,),
            {},
            {},
            operator_proof=operator_proof,
        )

    def sign_up_set(self, *, policy, operator_proof=None):
        """Set the sign-up policy (`POST /ops/sign-up`, `ops_sign_up_set`).

        Refused once the cluster's owner runs sign-up.

        **Who:** the ops token. **Operator signature:** answers `428 OPERATOR_SIGNATURE_REQUIRED` first; send it again with `operatorProof`, the operator key's signature over `details.message` and its `nonce`.

        Args:
            policy: body `policy`
            operator_proof: an operator seed (hex), a signer, a ready `operatorProof`, or False

        Returns: SignUpSet
        """
        return self._run(
            _OPS_SIGN_UP_SET, (), {}, {"policy": policy}, operator_proof=operator_proof
        )

    def status(self):
        """The node and the cluster (`GET /ops/status`, `ops_status`).

        Counts, Raft status and governance; never app state, names or credentials. `groups`: the app groups this node hosts; `apps` and `accounts` are counts; `rollback` is this node's rollback counter, `null` on a node without one.

        **Who:** the ops token.

        Returns: OpsStatusResponse200
        """
        return self._run(_OPS_STATUS, (), {}, None)

    def upgrade(
        self,
        *,
        app,
        bundle=UNSET,
        package=UNSET,
        version=UNSET,
        service=UNSET,
        migrate=UNSET,
        operator_proof=None,
    ):
        """Upgrade an app as the operator (`POST /ops/apps/{app}/upgrade`, `ops_upgrade`).

        To a release the operator installed, where the operator holds app upgrades. `error` is the app log's error value (`null` when it ran), free-form: the app's own.

        **Who:** the ops token. **Operator signature:** answers `428 OPERATOR_SIGNATURE_REQUIRED` first; send it again with `operatorProof`, the operator key's signature over `details.message` and its `nonce`.

        Args:
            app: path
            bundle: body `bundle`
            package: body `package`
            version: body `version`
            service: body `service`
            migrate: body `migrate`
            operator_proof: an operator seed (hex), a signer, a ready `operatorProof`, or False

        Returns: OpsUpgradeResponse200
        """
        return self._run(
            _OPS_UPGRADE,
            (app,),
            {},
            {
                "bundle": bundle,
                "package": package,
                "version": version,
                "service": service,
                "migrate": migrate,
            },
            operator_proof=operator_proof,
        )


_ORG_ALL_APPS = Operation(
    _org_all_apps, "org_all_apps", body=OrgAllAppsBody, answer="json", confirm=True
)
_APP_TEAM_LIST = Operation(
    _app_team_list,
    "app_team_list",
    body="none",
    answer="json",
)
_APP_TEAMS_SCOPE = Operation(
    _app_teams_scope,
    "app_teams_scope",
    body=AppTeamsScopeBody,
    answer="json",
    confirm=True,
)
_ORG_APPS = Operation(
    _org_apps,
    "org_apps",
    body="none",
    answer="json",
)
_ORG_BACKUP_KEY = Operation(
    _org_backup_key,
    "org_backup_key",
    body="none",
    answer="json",
)
_ORG_CAPABILITIES = Operation(
    _org_capabilities,
    "org_capabilities",
    body=OrgCapabilitiesBody,
    answer="json",
    confirm=True,
)
_ORG_CREATE = Operation(
    _org_create,
    "org_create",
    body=OrgCreateBody,
    answer="json",
)
_ORG_DELETE = Operation(
    _org_delete, "org_delete", body=ConfirmOnly, answer="json", confirm=True
)
_ORG_GET = Operation(
    _org_get,
    "org_get",
    body="none",
    answer="json",
)
_INVITE_CREATE = Operation(
    _invite_create, "invite_create", body=InviteCreateBody, answer="json", confirm=True
)
_INVITE_LIST = Operation(
    _invite_list,
    "invite_list",
    body="none",
    answer="json",
)
_INVITE_REDEEM = Operation(
    _invite_redeem,
    "invite_redeem",
    body="none",
    answer="json",
)
_INVITE_REVOKE = Operation(
    _invite_revoke, "invite_revoke", body=ConfirmOnly, answer="json", confirm=True
)
_MEMBER_METADATA = Operation(
    _member_metadata,
    "member_metadata",
    body=MemberMetadataBody,
    answer="json",
)
_MEMBER_REMOVE_TEAMS = Operation(
    _member_remove_teams,
    "member_remove_teams",
    body=MemberRemoveTeamsBody,
    answer="json",
    confirm=True,
)
_MEMBER_SET_TEAMS = Operation(
    _member_set_teams,
    "member_set_teams",
    body=MemberSetTeamsBody,
    answer="json",
    confirm=True,
)
_ORG_MEMBERS = Operation(
    _org_members,
    "org_members",
    body="none",
    answer="json",
)
_ORG_METADATA = Operation(
    _org_metadata,
    "org_metadata",
    body=OrgMetadataBody,
    answer="json",
)
_ORG_OUTBOUND_APPROVAL = Operation(
    _org_outbound_approval,
    "org_outbound_approval",
    body=OrgOutboundApprovalBody,
    answer="json",
    confirm=True,
)
_ORG_PUBLISHER = Operation(
    _org_publisher, "org_publisher", body=OrgPublisherBody, answer="json", confirm=True
)
_PUBLISHER_UPGRADE = Operation(
    _publisher_upgrade,
    "publisher_upgrade",
    body=PublisherUpgradeBody,
    answer="json",
    confirm=True,
)
_RECOVERY_CANCEL = Operation(
    _recovery_cancel, "recovery_cancel", body=ConfirmOnly, answer="json", confirm=True
)
_RECOVERY_FINISH = Operation(
    _recovery_finish, "recovery_finish", body=ConfirmOnly, answer="json", confirm=True
)
_RECOVERY_SET = Operation(
    _recovery_set, "recovery_set", body=RecoverySetBody, answer="json", confirm=True
)
_RECOVERY_START = Operation(
    _recovery_start,
    "recovery_start",
    body=RecoveryStartBody,
    answer="json",
)
_ORG_RELEASE_APPROVAL = Operation(
    _org_release_approval,
    "org_release_approval",
    body=OrgReleaseApprovalBody,
    answer="json",
    confirm=True,
)
_ORG_REMOVE_MEMBER = Operation(
    _org_remove_member,
    "org_remove_member",
    body=ConfirmOnly,
    answer="json",
    confirm=True,
)
_ORG_RENAME = Operation(
    _org_rename, "org_rename", body=OrgRenameBody, answer="json", confirm=True
)
_ORG_SET_BACKUP_KEY = Operation(
    _org_set_backup_key,
    "org_set_backup_key",
    body=OrgSetBackupKeyBody,
    answer="json",
    confirm=True,
)
_ORG_SET_MEMBER = Operation(
    _org_set_member,
    "org_set_member",
    body=OrgSetMemberBody,
    answer="json",
    confirm=True,
)
_TEAM_CREATE = Operation(
    _team_create,
    "team_create",
    body=TeamCreateBody,
    answer="json",
)
_TEAM_DELETE = Operation(
    _team_delete, "team_delete", body=ConfirmOnly, answer="json", confirm=True
)
_TEAM_GET = Operation(
    _team_get,
    "team_get",
    body="none",
    answer="json",
)
_TEAM_REMOVE_MEMBER = Operation(
    _team_remove_member,
    "team_remove_member",
    body=ConfirmOnly,
    answer="json",
    confirm=True,
)
_TEAM_SET_MEMBER = Operation(
    _team_set_member,
    "team_set_member",
    body=TeamSetMemberBody,
    answer="json",
    confirm=True,
)
_TEAM_UPDATE = Operation(
    _team_update,
    "team_update",
    body=TeamUpdateBody,
    answer="json",
)
_ORG_UPGRADE = Operation(
    _org_upgrade, "org_upgrade", body=OrgUpgradeBody, answer="json", confirm=True
)
_ORG_UPGRADE_RETRY = Operation(
    _org_upgrade_retry,
    "org_upgrade_retry",
    body=OrgUpgradeRetryBody,
    answer="json",
    confirm=True,
    operator=True,
)
_ORG_UPGRADE_STATUS = Operation(
    _org_upgrade_status,
    "org_upgrade_status",
    body="none",
    answer="json",
)
_ORG_USAGE = Operation(
    _org_usage,
    "org_usage",
    body="none",
    answer="json",
)


class Orgs(Group):
    """`client.orgs`: the operations core's document groups as `orgs`."""

    def all_apps(self, *, org, account, all_apps, confirm=None):
        """Give a member every app added later (`POST /orgs/{org}/all-apps`, `org_all_apps`).

        Or stop it (`allApps: false`); apps already there are not touched. The owner or an admin; `412` first.

        **Who:** a passkey session (an API key is refused). **Step-up:** answers `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over `details.challenge`.

        Args:
            org: path
            account: body `account`
            all_apps: body `allApps`
            confirm: a passkey for the step-up, a ready `confirm`, or False

        Returns: OrgAllAppsResponse200
        """
        return self._run(
            _ORG_ALL_APPS,
            (org,),
            {},
            {"account": account, "allApps": all_apps},
            confirm=confirm,
        )

    def app_team_list(self, *, org):
        """The organization's app teams (`GET /orgs/{org}/app-teams`, `app_team_list`).

        A member: each app team's package, id, name, `join`, member count, default capabilities, and whether the caller is in it or manages it (a guest: only their own).

        **Who:** signed in (a session or an API key).

        Args:
            org: path

        Returns: AppTeamListResponse200
        """
        return self._run(_APP_TEAM_LIST, (org,), {}, None)

    def app_teams_scope(self, *, org, apps=UNSET, shared=UNSET, confirm=None):
        """Give apps app teams, or open or close one (`POST /orgs/{org}/app-teams`, `app_teams_scope`).

        The owner or an admin; `keep` names who stays when a team closes; `childTeams` says whether the app team's members make restricted teams below it (private channels), adding or removing `CREATE_CHILD_TEAM` (absent: as they are). Naming `join` or `childTeams` on any package leaves `shared` unset to mean no other team. `412` first.

        **Who:** a passkey session (an API key is refused). **Step-up:** answers `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over `details.challenge`.

        Args:
            org: path
            apps: body `apps`
            shared: body `shared`
            confirm: a passkey for the step-up, a ready `confirm`, or False

        Returns: AppTeamsScopeResponse200
        """
        return self._run(
            _APP_TEAMS_SCOPE,
            (org,),
            {},
            {"apps": apps, "shared": shared},
            confirm=confirm,
        )

    def apps(self, *, org, after=UNSET, limit=UNSET):
        """The organization's apps (`GET /orgs/{org}/apps`, `org_apps`).

        As `GET /auth/apps`, for the apps the caller reaches or manages; `404` to a non-member.

        **Who:** signed in (a session or an API key).

        Args:
            org: path
            after: query
            limit: query

        Returns: OrgAppsResponse200
        """
        return self._run(_ORG_APPS, (org,), {"after": after, "limit": limit}, None)

    def backup_key(self, *, org):
        """The organization's backup key (`GET /orgs/{org}/backup-key`, `org_backup_key`).

        The X25519 public key its apps' recovery-disk backups are sealed to (`key`, `null` while none is set), and how many apps that covers. Any member; `404` outside the organization.

        **Who:** an account holder (a passkey session not bound to an app's frontend).

        Args:
            org: path

        Returns: BackupKeyAnswer
        """
        return self._run(_ORG_BACKUP_KEY, (org,), {}, None)

    def capabilities(
        self, *, org, account=UNSET, team=UNSET, capabilities, confirm=None
    ):
        """Set capabilities (`POST /orgs/{org}/capabilities`, `org_capabilities`).

        A member's (`account`), in a team (`team`), or a team's default; the capability bits. `412` first.

        **Who:** a passkey session (an API key is refused). **Step-up:** answers `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over `details.challenge`.

        Args:
            org: path
            account: body `account`
            team: body `team`
            capabilities: body `capabilities`
            confirm: a passkey for the step-up, a ready `confirm`, or False

        Returns: OrgCapabilitiesResponse200
        """
        return self._run(
            _ORG_CAPABILITIES,
            (org,),
            {},
            {"account": account, "team": team, "capabilities": capabilities},
            confirm=confirm,
        )

    def create(self, *, name, recovery_key=UNSET, capabilities=UNSET):
        """Create an organization (`POST /orgs`, `org_create`).

        `recoveryKey` is the public half of an organization recovery key. `capabilities` (bits) is what members can do unless set for them, in place of the default; `409 FEATURE_NOT_ACTIVE` below cluster version 1.0.0-rc.29. Every account also has a personal organization from sign-up (`personal` in `/auth/me`), which this does not make.

        **Who:** a passkey session (an API key is refused).

        Args:
            name: body `name`
            recovery_key: body `recoveryKey`
            capabilities: body `capabilities`

        Returns: OrgCreateResponse200
        """
        return self._run(
            _ORG_CREATE,
            (),
            {},
            {"name": name, "recoveryKey": recovery_key, "capabilities": capabilities},
        )

    def delete(self, *, org, confirm=None):
        """Delete an organization (`DELETE /orgs/{org}`, `org_delete`).

        With its teams, invites and apps; the owner. `apps`: the apps deleted with it.

        **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over `details.challenge`.

        Args:
            org: path
            confirm: a passkey for the step-up, a ready `confirm`, or False

        Returns: OrgDeleteResponse200
        """
        return self._run(_ORG_DELETE, (org,), {}, {}, confirm=confirm)

    def get(self, *, org):
        """One organization (`GET /orgs/{org}`, `org_get`).

        As the directory records it, with only the teams the caller reaches or manages and the people they see; and its `mode` (from its app teams), the accounts that may approve its releases (`releaseApprovers`), the keys it publishes under (`publishes`), and `newWorkStopped`: the release whose passed deadline stops new work in it (`after`, RFC 3339), `null` when none does. Those four are absent from a node older than they are (1.0.0-rc.1 sends none of them).

        **Who:** signed in (a session or an API key).

        Args:
            org: path

        Returns: Org
        """
        return self._run(_ORG_GET, (org,), {}, None)

    def invite_create(
        self,
        *,
        org,
        role,
        teams=UNSET,
        ttl_secs=UNSET,
        uses=UNSET,
        all_apps=UNSET,
        capabilities=UNSET,
        confirm=None,
    ):
        """Make an invite (`POST /orgs/{org}/invites`, `invite_create`).

        A token for `role` and `teams`, `uses` times within `ttlSecs`; `allApps` (an admin's) also gives every app added later; `capabilities` (bits) is what a new member gets as their own setting, no more than the creator holds (`403`), and `409 FEATURE_NOT_ACTIVE` below cluster version 1.0.0-rc.29. `412` first.

        **Who:** a passkey session (an API key is refused). **Step-up:** answers `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over `details.challenge`.

        Args:
            org: path
            role: body `role`
            teams: body `teams`
            ttl_secs: body `ttlSecs`
            uses: body `uses`
            all_apps: body `allApps`
            capabilities: body `capabilities`
            confirm: a passkey for the step-up, a ready `confirm`, or False

        Returns: InviteCreateResponse200
        """
        return self._run(
            _INVITE_CREATE,
            (org,),
            {},
            {
                "role": role,
                "teams": teams,
                "ttlSecs": ttl_secs,
                "uses": uses,
                "allApps": all_apps,
                "capabilities": capabilities,
            },
            confirm=confirm,
        )

    def invite_list(self, *, org):
        """The organization's invites (`GET /orgs/{org}/invites`, `invite_list`).

        Their keys and terms, never a token: `expiresAt` Unix seconds; `createdBy` and `createdAt` are `null` on an invite from before they were kept.

        **Who:** a passkey session (an API key is refused).

        Args:
            org: path

        Returns: InviteListResponse200
        """
        return self._run(_INVITE_LIST, (org,), {}, None)

    def invite_redeem(self, *, token):
        """Join with an invite (`POST /invites/{token}/redeem`, `invite_redeem`).

        No passkey prompt: the node signs the redemption with the token's key. Answers the organization joined.

        **Who:** a passkey session (an API key is refused).

        Args:
            token: path

        Returns: InviteRedeemResponse200
        """
        return self._run(_INVITE_REDEEM, (token,), {}, None)

    def invite_revoke(self, *, org, key, confirm=None):
        """Revoke an invite (`DELETE /orgs/{org}/invites/{key}`, `invite_revoke`).

        **Who:** a passkey session (an API key is refused). **Step-up:** answers `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over `details.challenge`.

        Args:
            org: path
            key: path
            confirm: a passkey for the step-up, a ready `confirm`, or False

        Returns: InviteRevokeResponse200
        """
        return self._run(
            _INVITE_REVOKE,
            (
                org,
                key,
            ),
            {},
            {},
            confirm=confirm,
        )

    def member_metadata(self, *, org, account, metadata):
        """Set a member's metadata (`POST /orgs/{org}/members/{account}/metadata`, `member_metadata`).

        `metadata` is the caller's own JSON, free-form, kept as given.

        **Who:** signed in (a session or an API key).

        Args:
            org: path
            account: path
            metadata: body `metadata`

        Returns: MemberMetadataResponse200
        """
        return self._run(
            _MEMBER_METADATA,
            (
                org,
                account,
            ),
            {},
            {"metadata": metadata},
        )

    def member_remove_teams(self, *, org, account, teams, confirm=None):
        """Remove a member from several teams, or leave them (`DELETE /orgs/{org}/members/{account}/teams`, `member_remove_teams`).

        Each judged as `DELETE /orgs/{org}/teams/{team}/members/{account}` would judge it, and none taken unless every one may be. 1 to 64 distinct teams; one passkey, `412` first.

        **Who:** a passkey session (an API key is refused). **Step-up:** answers `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over `details.challenge`.

        Args:
            org: path
            account: path
            teams: body `teams`
            confirm: a passkey for the step-up, a ready `confirm`, or False

        Returns: MemberRemoveTeamsResponse200
        """
        return self._run(
            _MEMBER_REMOVE_TEAMS,
            (
                org,
                account,
            ),
            {},
            {"teams": teams},
            confirm=confirm,
        )

    def member_set_teams(self, *, org, account, teams, confirm=None):
        """Set a member's rows in several teams (`POST /orgs/{org}/members/{account}/teams`, `member_set_teams`).

        Each team judged as `POST /orgs/{org}/teams/{team}/members` would judge it, and none set unless every one may be (a row's absent `role` is `member`; absent `capabilities`, a new row takes the defaults and an existing row keeps its own). 1 to 64 distinct teams; one passkey, `412` first.

        **Who:** a passkey session (an API key is refused). **Step-up:** answers `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over `details.challenge`.

        Args:
            org: path
            account: path
            teams: body `teams`
            confirm: a passkey for the step-up, a ready `confirm`, or False

        Returns: MemberSetTeamsResponse200
        """
        return self._run(
            _MEMBER_SET_TEAMS,
            (
                org,
                account,
            ),
            {},
            {"teams": teams},
            confirm=confirm,
        )

    def members(self, *, org, detail=UNSET):
        """Members' lockout risk (`GET /orgs/{org}/members`, `org_members`).

        `detail=security` (the only listing: without it, or with another value, `400 BAD_REQUEST`), the owner or an admin: each member's passkeys (`1`, `2` or `"3+"`), recovery code, last active UTC day and sessions. `403` below admin, `404` outside the organization.

        **Who:** an account holder (a passkey session not bound to an app's frontend).

        Args:
            org: path
            detail: query

        Returns: OrgMembersResponse200
        """
        return self._run(
            _ORG_MEMBERS,
            (org,),
            {"detail": Operation.enum(OrgMembersDetail, detail)},
            None,
        )

    def metadata(self, *, org, metadata):
        """Set an organization's metadata (`POST /orgs/{org}/metadata`, `org_metadata`).

        `metadata` is the caller's own JSON, free-form, kept as given.

        **Who:** signed in (a session or an API key).

        Args:
            org: path
            metadata: body `metadata`

        Returns: OrgMetadataResponse200
        """
        return self._run(_ORG_METADATA, (org,), {}, {"metadata": metadata})

    def outbound_approval(
        self, *, org, package, prefixes=UNSET, replace=UNSET, confirm=None
    ):
        """Approve outbound prefixes for the organization's apps (`POST /orgs/{org}/outbound`, `org_outbound_approval`).

        The owner; `412` first.

        **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over `details.challenge`.

        Args:
            org: path
            package: body `package`
            prefixes: body `prefixes`
            replace: body `replace`
            confirm: a passkey for the step-up, a ready `confirm`, or False

        Returns: OutboundApproved
        """
        return self._run(
            _ORG_OUTBOUND_APPROVAL,
            (org,),
            {},
            {"package": package, "prefixes": prefixes, "replace": replace},
            confirm=confirm,
        )

    def publisher(self, *, org, signer_id, public_key, signature, confirm=None):
        """Bind a package signing key (`POST /orgs/{org}/publishers`, `org_publisher`).

        The key's signature over the binding proves the organization holds it; then its `MANAGE_APPLICATION` holders upgrade the apps of packages that name `publisher`. Answers the key's binding generation now.

        **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over `details.challenge`.

        Args:
            org: path
            signer_id: body `signerId`
            public_key: body `publicKey`
            signature: body `signature`
            confirm: a passkey for the step-up, a ready `confirm`, or False

        Returns: OrgPublisherResponse200
        """
        return self._run(
            _ORG_PUBLISHER,
            (org,),
            {},
            {"signerId": signer_id, "publicKey": public_key, "signature": signature},
            confirm=confirm,
        )

    def publisher_upgrade(self, *, org, package, version, migrate=UNSET, confirm=None):
        """Upgrade every instance of a publisher's package (`POST /orgs/{org}/packages/{package}/upgrade`, `publisher_upgrade`).

        An upgrade run over every app on the cluster that runs the package, by its publisher's `MANAGE_APPLICATION` holders.

        **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over `details.challenge`.

        Args:
            org: path
            package: path
            version: body `version`
            migrate: body `migrate`
            confirm: a passkey for the step-up, a ready `confirm`, or False

        Returns: UpgradeRun
        """
        return self._run(
            _PUBLISHER_UPGRADE,
            (
                org,
                package,
            ),
            {},
            {"version": version, "migrate": migrate},
            confirm=confirm,
        )

    def recovery_cancel(self, *, org, confirm=None):
        """Cancel an organization recovery (`POST /orgs/{org}/recovery/cancel`, `recovery_cancel`).

        The owner; `412` first.

        **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over `details.challenge`.

        Args:
            org: path
            confirm: a passkey for the step-up, a ready `confirm`, or False

        Returns: RecoveryCancelResponse200
        """
        return self._run(_RECOVERY_CANCEL, (org,), {}, {}, confirm=confirm)

    def recovery_finish(self, *, org, confirm=None):
        """Finish an organization recovery (`POST /orgs/{org}/recovery/finish`, `recovery_finish`).

        The account that started it, once `readyAt` passed; `412` first.

        **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over `details.challenge`.

        Args:
            org: path
            confirm: a passkey for the step-up, a ready `confirm`, or False

        Returns: RecoveryFinishResponse200
        """
        return self._run(_RECOVERY_FINISH, (org,), {}, {}, confirm=confirm)

    def recovery_set(self, *, org, public_key=UNSET, confirm=None):
        """Set the organization's recovery key (`POST /orgs/{org}/recovery`, `recovery_set`).

        Set or rotate it (`publicKey`: base64url Ed25519 public key), or remove it (`publicKey: null`; an absent `publicKey` removes it too); the owner; cancels a recovery under way. `412` first.

        **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over `details.challenge`.

        Args:
            org: path
            public_key: body `publicKey`
            confirm: a passkey for the step-up, a ready `confirm`, or False

        Returns: RecoverySetResponse200
        """
        return self._run(
            _RECOVERY_SET, (org,), {}, {"publicKey": public_key}, confirm=confirm
        )

    def recovery_start(self, *, org, nonce, signature):
        """Start an organization recovery (`POST /orgs/{org}/recovery/start`, `recovery_start`).

        The recovery seed's signature handing the organization to the caller.

        **Who:** an account holder (a passkey session not bound to an app's frontend).

        Args:
            org: path
            nonce: body `nonce`
            signature: body `signature`

        Returns: RecoveryStartResponse200
        """
        return self._run(
            _RECOVERY_START, (org,), {}, {"nonce": nonce, "signature": signature}
        )

    def release_approval(self, *, org, version, approved=UNSET, confirm=None):
        """Approve a mero-tee release for the organization's apps (`POST /orgs/{org}/releases`, `org_release_approval`).

        The owner; `412` first.

        **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over `details.challenge`.

        Args:
            org: path
            version: body `version`
            approved: body `approved`
            confirm: a passkey for the step-up, a ready `confirm`, or False

        Returns: ApprovedReleases
        """
        return self._run(
            _ORG_RELEASE_APPROVAL,
            (org,),
            {},
            {"version": version, "approved": approved},
            confirm=confirm,
        )

    def remove_member(self, *, org, account, confirm=None):
        """Remove a member, or leave (`DELETE /orgs/{org}/members/{account}`, `org_remove_member`).

        `412` first; the retry's JSON body is `{ confirm }`.

        **Who:** a passkey session (an API key is refused). **Step-up:** answers `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over `details.challenge`.

        Args:
            org: path
            account: path
            confirm: a passkey for the step-up, a ready `confirm`, or False

        Returns: OrgRemoveMemberResponse200
        """
        return self._run(
            _ORG_REMOVE_MEMBER,
            (
                org,
                account,
            ),
            {},
            {},
            confirm=confirm,
        )

    def rename(self, *, org, name, confirm=None):
        """Rename an organization (`POST /orgs/{org}/name`, `org_rename`).

        The owner or an admin, to a name that fits as a new organization's would; `412` first.

        **Who:** a passkey session (an API key is refused). **Step-up:** answers `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over `details.challenge`.

        Args:
            org: path
            name: body `name`
            confirm: a passkey for the step-up, a ready `confirm`, or False

        Returns: OrgRenameResponse200
        """
        return self._run(_ORG_RENAME, (org,), {}, {"name": name}, confirm=confirm)

    def set_backup_key(self, *, org, public_key, confirm=None):
        """Set the organization's backup key (`POST /orgs/{org}/backup-key`, `org_set_backup_key`).

        `publicKey`: 64 lowercase hex, an X25519 public key whose private half its holder keeps (never a node). Those who upgrade all the organization's apps (org-wide `MANAGE_APPLICATION`), where owners upgrade apps; replaces the key before. `412` first.

        **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over `details.challenge`.

        Args:
            org: path
            public_key: body `publicKey`
            confirm: a passkey for the step-up, a ready `confirm`, or False

        Returns: BackupKeyAnswer
        """
        return self._run(
            _ORG_SET_BACKUP_KEY, (org,), {}, {"publicKey": public_key}, confirm=confirm
        )

    def set_member(self, *, org, account, role, confirm=None):
        """Set a member's role (`POST /orgs/{org}/members`, `org_set_member`).

        `owner` moves ownership. `412` first.

        **Who:** a passkey session (an API key is refused). **Step-up:** answers `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over `details.challenge`.

        Args:
            org: path
            account: body `account`
            role: body `role`
            confirm: a passkey for the step-up, a ready `confirm`, or False

        Returns: OrgSetMemberResponse200
        """
        return self._run(
            _ORG_SET_MEMBER,
            (org,),
            {},
            {"account": account, "role": role},
            confirm=confirm,
        )

    def team_create(self, *, org, name, parent=UNSET, join):
        """Create a team (`POST /orgs/{org}/teams`, `team_create`).

        An admin, a lead of `parent`, a member with `CREATE_TEAM` at the top, or a member who holds `CREATE_CHILD_TEAM` under `parent` (a restricted team only). A package-bound session: only its own package's teams (its app team and the teams below it): `parent` one of them.

        **Who:** a passkey session (an API key is refused).

        Args:
            org: path
            name: body `name`
            parent: body `parent`
            join: body `join`

        Returns: TeamCreateResponse200
        """
        return self._run(
            _TEAM_CREATE, (org,), {}, {"name": name, "parent": parent, "join": join}
        )

    def team_delete(self, *, org, team, confirm=None):
        """Delete a team (`DELETE /orgs/{org}/teams/{team}`, `team_delete`).

        With the teams and apps below it. A package-bound session: only its own package's teams (its app team and the teams below it): a team below its app team whose apps are all its package's.

        **Who:** a passkey session (an API key is refused). **Step-up:** answers `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over `details.challenge`.

        Args:
            org: path
            team: path
            confirm: a passkey for the step-up, a ready `confirm`, or False

        Returns: TeamDeleteResponse200
        """
        return self._run(
            _TEAM_DELETE,
            (
                org,
                team,
            ),
            {},
            {},
            confirm=confirm,
        )

    def team_get(self, *, org, team):
        """One team (`GET /orgs/{org}/teams/{team}`, `team_get`).

        **Who:** signed in (a session or an API key).

        Args:
            org: path
            team: path

        Returns: TeamView
        """
        return self._run(
            _TEAM_GET,
            (
                org,
                team,
            ),
            {},
            None,
        )

    def team_remove_member(self, *, org, team, account, confirm=None):
        """Remove a team member, or leave (`DELETE /orgs/{org}/teams/{team}/members/{account}`, `team_remove_member`).

        A package-bound session: only its own package's teams (its app team and the teams below it).

        **Who:** a passkey session (an API key is refused). **Step-up:** answers `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over `details.challenge`.

        Args:
            org: path
            team: path
            account: path
            confirm: a passkey for the step-up, a ready `confirm`, or False

        Returns: TeamRemoveMemberResponse200
        """
        return self._run(
            _TEAM_REMOVE_MEMBER,
            (
                org,
                team,
                account,
            ),
            {},
            {},
            confirm=confirm,
        )

    def team_set_member(
        self,
        *,
        org,
        team,
        account,
        lead=UNSET,
        role=UNSET,
        capabilities=UNSET,
        confirm=None,
    ):
        """Add or change a team member (`POST /orgs/{org}/teams/{team}/members`, `team_set_member`).

        A package-bound session: only its own package's teams (its app team and the teams below it).

        **Who:** a passkey session (an API key is refused). **Step-up:** answers `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over `details.challenge`.

        Args:
            org: path
            team: path
            account: body `account`
            lead: body `lead`
            role: body `role`
            capabilities: body `capabilities`
            confirm: a passkey for the step-up, a ready `confirm`, or False

        Returns: TeamSetMemberResponse200
        """
        return self._run(
            _TEAM_SET_MEMBER,
            (
                org,
                team,
            ),
            {},
            {
                "account": account,
                "lead": lead,
                "role": role,
                "capabilities": capabilities,
            },
            confirm=confirm,
        )

    def team_update(
        self, *, org, team, name=UNSET, join=UNSET, parent=UNSET, metadata=UNSET
    ):
        """Update a team (`POST /orgs/{org}/teams/{team}`, `team_update`).

        Rename, open or restrict, move under another parent, or set its metadata (the caller's own JSON, free-form, kept as given). `parent` absent leaves the team where it is; `parent: null` (an explicit null) moves it to the top; a team id moves it under that team. A package-bound session: only its own package's teams (its app team and the teams below it): a team below its app team, moved only within.

        **Who:** a passkey session (an API key is refused).

        Args:
            org: path
            team: path
            name: body `name`
            join: body `join`
            parent: body `parent`
            metadata: body `metadata`

        Returns: TeamUpdateResponse200
        """
        return self._run(
            _TEAM_UPDATE,
            (
                org,
                team,
            ),
            {},
            {"name": name, "join": join, "parent": parent, "metadata": metadata},
        )

    def upgrade(
        self,
        *,
        org,
        package,
        version,
        team=UNSET,
        cascade=UNSET,
        migrate=UNSET,
        confirm=None,
    ):
        """Start an upgrade run (`POST /orgs/{org}/upgrade`, `org_upgrade`).

        Upgrade every app of the organization (or of `team`, with `cascade` its subtree) that runs `package` to `version`, with one passkey assertion over the whole run; whoever holds `MANAGE_APPLICATION` there. Each app is upgraded as `POST /apps/{app}/upgrade` would; `GET /orgs/{org}/upgrades/{run}` follows it.

        **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over `details.challenge`.

        Args:
            org: path
            package: body `package`
            version: body `version`
            team: body `team`
            cascade: body `cascade`
            migrate: body `migrate`
            confirm: a passkey for the step-up, a ready `confirm`, or False

        Returns: UpgradeRun
        """
        return self._run(
            _ORG_UPGRADE,
            (org,),
            {},
            {
                "package": package,
                "version": version,
                "team": team,
                "cascade": cascade,
                "migrate": migrate,
            },
            confirm=confirm,
        )

    def upgrade_retry(self, *, org, run, confirm=None, operator_proof=None):
        """Retry an upgrade run's failed apps (`POST /orgs/{org}/upgrades/{run}/retry`, `org_upgrade_retry`).

        As a new run under the same terms: a passkey step-up for a member's run, the operator's signature for an operator's run (with the ops token).

        **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over `details.challenge`. **Operator signature:** answers `428 OPERATOR_SIGNATURE_REQUIRED` first; send it again with `operatorProof`, the operator key's signature over `details.message` and its `nonce`.

        Args:
            org: path
            run: path
            confirm: a passkey for the step-up, a ready `confirm`, or False
            operator_proof: an operator seed (hex), a signer, a ready `operatorProof`, or False

        Returns: UpgradeRun
        """
        return self._run(
            _ORG_UPGRADE_RETRY,
            (
                org,
                run,
            ),
            {},
            {},
            confirm=confirm,
            operator_proof=operator_proof,
        )

    def upgrade_status(self, *, org, run):
        """An upgrade run's progress (`GET /orgs/{org}/upgrades/{run}`, `org_upgrade_status`).

        A member of the organization.

        **Who:** signed in (a session or an API key).

        Args:
            org: path
            run: path

        Returns: UpgradeRun
        """
        return self._run(
            _ORG_UPGRADE_STATUS,
            (
                org,
                run,
            ),
            {},
            None,
        )

    def usage(self, *, org):
        """An organization's usage (`GET /orgs/{org}/usage`, `org_usage`).

        An admin: how many apps the organization develops, what their runs used today (decimal strings) and the cluster's quotas; a member gets `403`.

        **Who:** signed in (a session or an API key).

        Args:
            org: path

        Returns: OrgUsageResponse200
        """
        return self._run(_ORG_USAGE, (org,), {}, None)


_BUNDLE_PUT = Operation(
    _bundle_put,
    "bundle_put",
    body="bytes",
    answer="json",
)
_PACKAGE_CAPABILITIES = Operation(
    _package_capabilities,
    "package_capabilities",
    body="none",
    answer="json",
)
_PACKAGE_GET = Operation(
    _package_get,
    "package_get",
    body="none",
    answer="json",
)
_PACKAGE_INSTALL = Operation(
    _package_install,
    "package_install",
    body=PackageInstallBody,
    answer="json",
)
_PACKAGE_LIST = Operation(
    _package_list,
    "package_list",
    body="none",
    answer="json",
)
_ORIGINS_ADD = Operation(
    _origins_add, "origins_add", body=OriginsAddBody, answer="json", confirm=True
)
_ORIGINS_LIST = Operation(
    _origins_list,
    "origins_list",
    body="none",
    answer="json",
)
_ORIGINS_REMOVE = Operation(
    _origins_remove,
    "origins_remove",
    body=OriginsRemoveBody,
    answer="json",
    confirm=True,
)
_PACKAGE_PIPELINES = Operation(
    _package_pipelines,
    "package_pipelines",
    body="none",
    answer="json",
)
_PACKAGE_UNINSTALL = Operation(
    _package_uninstall,
    "package_uninstall",
    body="none",
    answer="json",
)
_PACKAGE_UPLOAD = Operation(
    _package_upload,
    "package_upload",
    body="bytes",
    answer="json",
)
_PACKAGE_VERSION_UNINSTALL = Operation(
    _package_version_uninstall,
    "package_version_uninstall",
    body="none",
    answer="json",
)


class Packages(Group):
    """`client.packages`: the operations core's document groups as `packages`."""

    def bundle_put(self, *, body):
        """Upload a raw component (`POST /bundles`, `bundle_put`).

        A `calimero:app` component, 16 MiB at most; only on a node started with `--allow-unsigned-bundles` or `--dev`. A WASM core module (SDK 0.12) is refused `400 UNSUPPORTED_BUNDLE`.

        **Who:** an installer (an account holder, or an unscoped API key that is not read-only).

        Args:
            body: the raw bytes

        Returns: BundlePutResponse200
        """
        return self._run(_BUNDLE_PUT, (), {}, body)

    def capabilities(self, *, package, version, service=UNSET):
        """A release's capabilities (`GET /packages/{package}/{version}/capabilities`, `package_capabilities`).

        What the release is and may do, read from its signed manifest and the ABI its service component embeds, before any app of it exists: `bundle` is the service WASM's hash, `outbound` the sorted prefixes it may call out to and `abi` the embedded `calimero:abi` verbatim (`null` when it embeds none). `service` names the service whose card to read (default: the release's only one). A session bound to the package reads every release of it. `404` for a release the caller may not see, `400 NO_SUCH_SERVICE` for an unknown service or no `service` on a release with several, `400 BAD_REQUEST` for an over-long `service`.

        **Who:** signed in (a session or an API key).

        Args:
            package: path
            version: path
            service: query

        Returns: PackageCapabilitiesResponse200
        """
        return self._run(
            _PACKAGE_CAPABILITIES,
            (
                package,
                version,
            ),
            {"service": service},
            None,
        )

    def get(self, *, package):
        """One package (`GET /packages/{package}`, `package_get`).

        The versions the caller may see; `404` when none.

        **Who:** signed in (a session or an API key).

        Args:
            package: path

        Returns: Package
        """
        return self._run(_PACKAGE_GET, (package,), {}, None)

    def install(self, *, package, version):
        """Install a release from the registry (`POST /packages/install`, `package_install`).

        **Who:** an installer (an account holder, or an unscoped API key that is not read-only).

        Args:
            package: body `package`
            version: body `version`

        Returns: PackageInstallResponse200
        """
        return self._run(
            _PACKAGE_INSTALL, (), {}, {"package": package, "version": version}
        )

    def list(self):
        """Installed packages (`GET /packages`, `package_list`).

        Only the releases the caller may see.

        **Who:** signed in (a session or an API key).

        Returns: PackageListResponse200
        """
        return self._run(_PACKAGE_LIST, (), {}, None)

    def origins_add(self, *, package, origin, rp_id=UNSET, served=UNSET, confirm=None):
        """Register a frontend origin (`POST /package-origins/{package}`, `origins_add`).

        By whoever manages the package, with a passkey step-up over the change. Answers the origin as registered (the one already there, when the same origin is registered again).

        **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over `details.challenge`.

        Args:
            package: path
            origin: body `origin`
            rp_id: body `rpId`
            served: body `served`
            confirm: a passkey for the step-up, a ready `confirm`, or False

        Returns: PackageOrigin
        """
        return self._run(
            _ORIGINS_ADD,
            (package,),
            {},
            {"origin": origin, "rpId": rp_id, "served": served},
            confirm=confirm,
        )

    def origins_list(self, *, package):
        """A package's frontend origins (`GET /package-origins/{package}`, `origins_list`).

        The custom origins (and rp-ids) the package's manager registered for its frontend.

        **Who:** signed in (a session or an API key).

        Args:
            package: path

        Returns: OriginsListResponse200
        """
        return self._run(_ORIGINS_LIST, (package,), {}, None)

    def origins_remove(self, *, package, origin, confirm=None):
        """Remove a frontend origin (`DELETE /package-origins/{package}`, `origins_remove`).

        **Who:** an account holder (a passkey session not bound to an app's frontend). **Step-up:** answers `412 CONFIRM_REQUIRED` first; send the same request again with `confirm`, a passkey assertion over `details.challenge`.

        Args:
            package: path
            origin: body `origin`
            confirm: a passkey for the step-up, a ready `confirm`, or False

        Returns: OriginsRemoveResponse200
        """
        return self._run(
            _ORIGINS_REMOVE, (package,), {}, {"origin": origin}, confirm=confirm
        )

    def pipelines(self, *, package, version):
        """A release's pipelines (`GET /packages/{package}/{version}/pipelines`, `package_pipelines`).

        The pipelines the release registers, by name: `hash`, `worstCaseFuel` (a decimal string), the `modules` they pin and the canonical `pipeline`. `404` for a release the caller may not see.

        **Who:** signed in (a session or an API key).

        Args:
            package: path
            version: path

        Returns: PackagePipelinesResponse200
        """
        return self._run(
            _PACKAGE_PIPELINES,
            (
                package,
                version,
            ),
            {},
            None,
        )

    def uninstall(self, *, package):
        """Uninstall a package (`DELETE /packages/{package}`, `package_uninstall`).

        Every version; refused while an app runs one.

        **Who:** an account holder (a passkey session not bound to an app's frontend).

        Args:
            package: path

        Returns: Uninstalled
        """
        return self._run(_PACKAGE_UNINSTALL, (package,), {}, None)

    def upload(self, *, body):
        """Upload a signed release (`POST /packages`, `package_upload`).

        A signed `.mpk`, checked as an install from the registry is (signature, artifact digests, `minRuntimeVersion`, the package's pinned signer, every service compiles). Only with `--dev`; otherwise `403 UPLOADS_DISABLED`.

        **Who:** an installer (an account holder, or an unscoped API key that is not read-only).

        Args:
            body: the raw bytes

        Returns: PackageUploadResponse200
        """
        return self._run(_PACKAGE_UPLOAD, (), {}, body)

    def version_uninstall(self, *, package, version):
        """Uninstall one version (`DELETE /packages/{package}/{version}`, `package_version_uninstall`).

        **Who:** an account holder (a passkey session not bound to an app's frontend).

        Args:
            package: path
            version: path

        Returns: Uninstalled
        """
        return self._run(
            _PACKAGE_VERSION_UNINSTALL,
            (
                package,
                version,
            ),
            {},
            None,
        )


_ATTESTATION = Operation(
    _attestation,
    "attestation",
    body="none",
    answer="json",
)
_HEALTH = Operation(
    _health,
    "health",
    body="none",
    answer="json",
)
_PUBLISHER_BINDING = Operation(
    _publisher_binding,
    "publisher_binding",
    body="none",
    answer="json",
)
_RELEASES_PUBLIC = Operation(
    _releases_public,
    "releases_public",
    body="none",
    answer="json",
)
_RESTORE_RECOVER_HOLDER = Operation(
    _restore_recover_holder,
    "restore_recover_holder",
    body=RestoreRecoverHolderBody,
    answer="json",
    operator=True,
)


class Public(Group):
    """`client.public`: the operations core's document groups as `public`."""

    def attestation(self, *, nonce, app=UNSET, app_hash=UNSET, purpose=UNSET):
        """The node's attestation (`GET /attestation`, `attestation`).

        In the clear. A TDX quote binding the node's transport key (or, with `purpose=receipts`, its receipt key and the governance; with `purpose=kms`, what it verified of its KMS; with `purpose=orchestrator`, the cluster identity this replica holds) to `nonce`; `app`/`appHash` bind an app's bundle hash: `app` only an app anyone may reach (public), any other answering `404` as an unknown id does, so it never says that a private app exists. A client checks it before it seals anything to the key. The node makes one quote at a time and lets a few requests wait for it: past that `503 BUSY` with `Retry-After: 1`, ask again then.

        **Who:** anyone.

        Args:
            nonce: query
            app: query
            app_hash: query
            purpose: query

        Returns: AttestationDoc
        """
        return self._run(
            _ATTESTATION,
            (),
            {
                "nonce": nonce,
                "app": app,
                "app_hash": app_hash,
                "purpose": Operation.enum(AttestationPurpose, purpose),
            },
            None,
        )

    def health(self):
        """Node health (`GET /health`, `health`).

        In the clear. The node's id, the system group's leader (`null`: none known), its `merod` release, the newest cluster version it supports (`clusterVersion`) and the cluster's rollback epoch (`null` on a node without a rollback counter).

        **Who:** anyone.

        Returns: HealthResponse200
        """
        return self._run(_HEALTH, (), {}, None)

    def publisher_binding(self, *, signer):
        """Whether a signing key is bound (`GET /publishers/{signer}`, `publisher_binding`).

        Whether a package signing key (`did:key`) is bound to an organization on this cluster (`bound`, never which), and its binding generation (`0`: never bound).

        **Who:** signed in (a session or an API key).

        Args:
            signer: path

        Returns: PublisherBindingResponse200
        """
        return self._run(_PUBLISHER_BINDING, (signer,), {}, None)

    def releases_public(self):
        """The cluster's releases (`GET /releases`, `releases_public`).

        The cluster's governance and the deadlines set on mero-tee releases, oldest release first: after `after` (RFC 3339; `afterSecs` Unix seconds) a scope that has not approved that release (or a newer one) starts no new work.

        **Who:** anyone.

        Returns: ReleasesPublicResponse200
        """
        return self._run(_RELEASES_PUBLIC, (), {}, None)

    def restore_recover_holder(
        self, *, holder, slice_, apps=UNSET, code=UNSET, operator_proof=None
    ):
        """Bring an organization's or account's apps back from its recovery files (`POST /recover/holder`, `restore_recover_holder`).

        Sent by the holder's own device, over the sealed channel: its newest `holder` file (`slice`), each app's file and the code files, in the receiving node's `--recovery-dir` (or `--recover-from`) under `org-<id>/` or `account-<id>/`, each with the data key the holder opened from it with its backup key (hex). The cluster admits it with the operator's signature over `cluster / __recover_holder / { holder, slice, apps: [{ app, file, sha256 }] }` (`428` first, naming it). Where owners upgrade apps, and only for an organization, account, apps and passkeys the cluster does not hold; the receiving node seeds each app, which answers `409 APP_RESTORING` until its replicas hold it. People sign in again with the passkeys they had.

        **Who:** anyone. **Operator signature:** answers `428 OPERATOR_SIGNATURE_REQUIRED` first; send it again with `operatorProof`, the operator key's signature over `details.message` and its `nonce`.

        Args:
            holder: body `holder`
            slice_: body `slice`
            apps: body `apps`
            code: body `code`
            operator_proof: an operator seed (hex), a signer, a ready `operatorProof`, or False

        Returns: RestoreRecoverHolderResponse200
        """
        return self._run(
            _RESTORE_RECOVER_HOLDER,
            (),
            {},
            {"holder": holder, "slice": slice_, "apps": apps, "code": code},
            operator_proof=operator_proof,
        )


# The client's attribute for each group.
GROUPS = {
    "apps": Apps,
    "auth": Auth,
    "consent": Consent,
    "deployments": Deployments,
    "ops": Ops,
    "orgs": Orgs,
    "packages": Packages,
    "public": Public,
}
