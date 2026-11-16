package com.calimero.mero.sample.explorer

import com.calimero.mero.AppAccess
import com.calimero.mero.ConsentRequest
import com.calimero.mero.ConsentScope
import com.calimero.mero.ConsentTeamMembership
import com.calimero.mero.Explicit
import com.calimero.mero.api.models.OrgRole
import com.calimero.mero.api.models.TeamJoin
import com.calimero.mero.api.models.TeamRole
import com.calimero.mero.crypto.Hex
import com.calimero.mero.sample.app.SampleViewModel
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonElement

// The SDK surface: one `SDKOperation` per public `CalimeroClient` method, and the facade's
// operations (`client.orgs.inviteCreate`, ..., named `group.method`). Each runs on the signed-in session; methods the node asks the account holder
// to confirm use the device's passkey. Arguments are entered as text (JSON where it is JSON);
// results are pretty-printed. Mirrors the Swift sample's operation list.

private fun hex(bytes: ByteArray): String = Hex.encode(bytes)

private fun json(raw: String): JsonElement = Json.parseToJsonElement(raw.trim().ifEmpty { "{}" })

private fun out(value: Any?): String =
    when (value) {
        null, Unit -> "OK"
        is JsonElement -> Fmt.pretty.encodeToString(JsonElement.serializer(), value)
        is ByteArray -> "${value.size} bytes"
        else -> value.toString()
    }

private fun op(
    id: String,
    category: String,
    name: String,
    summary: String,
    vararg fields: OpField,
    run: suspend (OpEnv, Map<String, String>) -> Any?,
) = SDKOperation(id, category, name, summary, fields.toList()) { env, values -> out(run(env, values)) }

private val app = OpField.line("app", "App id", "the app instance's id")
private val org = OpField.line("org", "Organization id")
private val team = OpField.line("team", "Team id")
private val account = OpField.line("account", "Account id")
private val args = OpField.json("args", "Arguments (JSON)")

private val identityOps =
    listOf(
        op("me", "Account", "auth.me", "GET /auth/me: the account, its passkeys and organizations") { e, _ -> e.client.auth.me() },
        op("usage", "Account", "auth.usage", "What the account uses, with the cluster's quotas") { e, _ -> e.client.auth.usage() },
        op("refresh", "Account", "refresh", "Swap the refresh token for a new session (single-flight)") { e, _ -> e.client.refresh().accountId },
        op("addPasskey", "Account", "addPasskey", "Add this device's passkey to the account") { e, _ -> e.client.addPasskey(e.passkey, approver = e.passkey) },
        op("labelPasskey", "Account", "auth.passkeyLabel", "Name one of the account's passkeys", OpField.line("id", "Credential id"), OpField.line("label", "Label")) { e, v ->
            e.client.auth.passkeyLabel(cred = v.v("id"), label = v.v("label"))
        },
        op("removePasskey", "Account", "auth.passkeyRemove", "Remove one of the account's passkeys", OpField.line("id", "Credential id")) { e, v -> e.client.auth.passkeyRemove(cred = v.v("id"), passkey = e.passkey) },
        op("attestation", "Account", "attestation", "The node's attestation document (display only)") { e, _ -> e.client.attestation().toString() },
        op("listApiKeys", "Account", "auth.apikeyList", "The account's API keys") { e, _ ->
            e.client.auth
                .apikeyList()
                .propertyKeys
                .joinToString("\n")
        },
        op("createApiKey", "Account", "auth.apikeyCreate", "Create an API key (shown once)", OpField.line("label", "Label"), OpField.line("ttl", "TTL (seconds)", "3600")) { e, v ->
            e.client.auth
                .apikeyCreate(label = v.opt("label"), ttlSecs = v.opt("ttl")?.toLong(), passkey = e.passkey)
                .toString()
        },
        op("revokeApiKey", "Account", "auth.apikeyRevoke", "Revoke an API key (passkey)", OpField.line("key", "Key id")) { e, v -> e.client.auth.apikeyRevoke(key = v.v("key"), passkey = e.passkey) },
        op("signOutEverywhere", "Account", "signOutEverywhere", "End every session of the account") { e, _ -> e.client.signOutEverywhere() },
    )

private val appOps =
    listOf(
        op("listApps", "Apps", "apps.list", "Apps this account reaches", OpField.line("package", "Package (optional)")) { e, v ->
            e.client.apps
                .list(`package` = v.opt("package"))
                .apps
                .joinToString("\n")
        },
        op("getApp", "Apps", "apps.get", "One app instance's record", app) { e, v -> e.client.apps.get(app = v.v("app")) },
        op("createApp", "Apps", "apps.create", "Create an app instance from a package release", OpField.line("name", "Name"), OpField.line("package", "Package", "com.calimero.chat"), OpField.line("version", "Version (optional)"), OpField.line("org", "Organization (optional)"), OpField.json("initArgs", "init arguments (JSON)")) { e, v ->
            e.client.apps
                .create(name = v.v("name"), `package` = v.v("package"), version = v.opt("version"), org = v.opt("org"), initArgs = json(v.v("initArgs")))
                .appId
        },
        op("call", "Apps", "apps.intents", "POST /apps/{app}/intents: call a method", app, OpField.line("method", "Method"), args) { e, v ->
            e.client.apps
                .intents(app = v.v("app"), method = v.v("method"), args = json(v.v("args")))
                .toString()
        },
        op("callConfirmed", "Apps", "apps.intents", "Call a method that needs the passkey", app, OpField.line("method", "Method"), args) { e, v ->
            e.client.apps
                .intents(app = v.v("app"), method = v.v("method"), args = json(v.v("args")), passkey = e.passkey)
                .toString()
        },
        op("view", "Apps", "apps.views", "POST /apps/{app}/views: run a read", app, OpField.line("method", "Method"), args) { e, v ->
            e.client.apps
                .views(app = v.v("app"), method = v.v("method"), args = json(v.v("args")))
                .toString()
        },
        op("deleteApp", "Apps", "apps.delete", "Delete an app instance", app) { e, v -> e.client.apps.delete(app = v.v("app")) },
        op("setAccess", "Apps", "apps.access", "Who may reach the app: owner, org, public or a team", app, OpField.line("team", "Team id (blank: org)")) { e, v ->
            e.client.apps.access(app = v.v("app"), access = v.opt("team")?.let { AppAccess.Team(it) } ?: AppAccess.Org)
        },
        op("setAppMetadata", "Apps", "apps.metadata", "Set the app instance's metadata", app, OpField.json("metadata", "Metadata (JSON)")) { e, v -> e.client.apps.metadata(app = v.v("app"), metadata = json(v.v("metadata"))) },
        op("appStorage", "Apps", "apps.storage", "What the app's storage holds", app) { e, v -> e.client.apps.storage(app = v.v("app")) },
        op("appAbi", "Apps", "apps.abi", "The ABI the app's bundle embeds", app) { e, v -> e.client.apps.abi(app = v.v("app")) },
        op("upgradeApp", "Apps", "apps.upgrade", "Move an app to another release (confirmed with the passkey)", app, OpField.line("version", "Version")) { e, v ->
            e.client.apps.upgrade(app = v.v("app"), version = v.v("version"), passkey = e.passkey)
        },
        op("transferApp", "Apps", "apps.transfer", "Hand the app to another account", app, OpField.line("to", "Account id")) { e, v -> e.client.apps.transfer(app = v.v("app"), to = v.v("to"), passkey = e.passkey) },
        op("presence", "Apps", "apps.presenceGet", "Who is present in the app now", app) { e, v ->
            e.client.apps
                .presenceGet(app = v.v("app"))
                .present
                .joinToString("\n")
        },
        op("publishPresence", "Apps", "apps.presencePublish", "Publish this device's presence", app, OpField.line("seq", "Sequence", "1"), OpField.line("state", "State (text)")) { e, v ->
            e.client.apps.presencePublish(app = v.v("app"), seq = v.v("seq").toLongOrNull() ?: 1, state = v.opt("state")?.encodeToByteArray()?.let { Explicit(hex(it)) })
        },
        op("listBlobs", "Apps", "apps.blobList", "The app's blobs", app) { e, v ->
            e.client.apps
                .blobList(app = v.v("app"))
                .blobs
                .joinToString("\n")
        },
        op("uploadBlob", "Apps", "uploadBlob", "Upload text as a blob", app, OpField.line("text", "Text")) { e, v -> e.client.uploadBlob(v.v("app"), v.v("text").encodeToByteArray()) },
        op("downloadBlob", "Apps", "apps.blobDownload", "Download a blob", app, OpField.line("id", "Blob id")) { e, v ->
            e.client.apps
                .blobDownload(app = v.v("app"), id = v.v("id"))
                .decodeToString()
        },
        op("blobInfo", "Apps", "apps.blobInfo", "A blob's id and size", app, OpField.line("id", "Blob id")) { e, v -> e.client.apps.blobInfo(app = v.v("app"), id = v.v("id")) },
        op("deleteBlob", "Apps", "apps.blobDelete", "Delete a blob", app, OpField.line("id", "Blob id")) { e, v -> e.client.apps.blobDelete(app = v.v("app"), id = v.v("id")) },
        op("aliases", "Apps", "auth.aliasList", "This account's aliases") { e, _ ->
            e.client.auth
                .aliasList()
                .aliases
        },
        op("setAlias", "Apps", "auth.aliasSet", "Name an app", OpField.line("alias", "Alias"), app) { e, v -> e.client.auth.aliasSet(alias = v.v("alias"), app = v.v("app")) },
        op("alias", "Apps", "auth.aliasGet", "The app an alias names", OpField.line("alias", "Alias")) { e, v ->
            e.client.auth
                .aliasGet(alias = v.v("alias"))
                .appId
        },
        op("deleteAlias", "Apps", "auth.aliasDelete", "Forget an alias", OpField.line("alias", "Alias")) { e, v -> e.client.auth.aliasDelete(alias = v.v("alias")) },
    )

private val packageOps =
    listOf(
        op("packages", "Packages", "packages.list", "Installed packages") { e, _ ->
            e.client.packages
                .list()
                .packages
                .joinToString("\n")
        },
        op("package", "Packages", "packages.get", "One installed package", OpField.line("package", "Package")) { e, v -> e.client.packages.get(`package` = v.v("package")) },
        op("installPackage", "Packages", "packages.install", "Install a release from the registry", OpField.line("package", "Package"), OpField.line("version", "Version")) { e, v ->
            e.client.packages.install(`package` = v.v("package"), version = v.v("version"))
        },
        op("uninstallPackage", "Packages", "packages.versionUninstall", "Uninstall a version or the package", OpField.line("package", "Package"), OpField.line("version", "Version (optional)")) { e, v ->
            v.opt("version")?.let { e.client.packages.versionUninstall(`package` = v.v("package"), version = it) } ?: e.client.packages.uninstall(`package` = v.v("package"))
        },
    )

private val orgOps =
    listOf(
        op("createOrg", "Organizations", "orgs.create", "Create an organization", OpField.line("name", "Name")) { e, v ->
            e.client.orgs
                .create(name = v.v("name"))
                .orgId
        },
        op("getOrg", "Organizations", "orgs.get", "An organization: members, teams, metadata", org) { e, v -> e.client.orgs.get(org = v.v("org")) },
        op("deleteOrg", "Organizations", "orgs.delete", "Delete an organization (passkey)", org) { e, v -> e.client.orgs.delete(org = v.v("org"), passkey = e.passkey) },
        op("setOrgMetadata", "Organizations", "orgs.metadata", "Set the organization's metadata", org, OpField.json("metadata", "Metadata (JSON)")) { e, v -> e.client.orgs.metadata(org = v.v("org"), metadata = json(v.v("metadata"))) },
        op("setMember", "Organizations", "orgs.setMember", "Add a member or change their role", org, account, OpField.line("role", "Role", "member")) { e, v ->
            e.client.orgs.setMember(org = v.v("org"), account = v.v("account"), role = checkNotNull(OrgRole.decode(v.v("role"))), passkey = e.passkey)
        },
        op("removeMember", "Organizations", "orgs.removeMember", "Remove a member (or leave: your own account)", org, account) { e, v -> e.client.orgs.removeMember(org = v.v("org"), account = v.v("account"), passkey = e.passkey) },
        op("setMemberMetadata", "Organizations", "orgs.memberMetadata", "Set a member's metadata", org, account, OpField.json("metadata", "Metadata (JSON)")) { e, v ->
            e.client.orgs.memberMetadata(org = v.v("org"), account = v.v("account"), metadata = json(v.v("metadata")))
        },
        op("setCapabilities", "Organizations", "orgs.capabilities", "Set capability bits", org, OpField.line("caps", "Bits", "4"), OpField.line("account", "Account (optional)")) { e, v ->
            e.client.orgs.capabilities(org = v.v("org"), capabilities = v.v("caps").toLong(), account = v.opt("account"), passkey = e.passkey)
        },
        op("invite", "Organizations", "orgs.inviteCreate", "Mint an invite token", org, OpField.line("role", "Role", "member")) { e, v ->
            e.client.orgs
                .inviteCreate(org = v.v("org"), role = checkNotNull(OrgRole.decode(v.v("role"))), teams = emptyList(), passkey = e.passkey)
                .token
        },
        op("redeemInvite", "Organizations", "orgs.inviteRedeem", "Redeem an invite token", OpField.line("token", "Token", "inv_…")) { e, v -> e.client.orgs.inviteRedeem(token = v.v("token")) },
        op("createTeam", "Organizations", "orgs.teamCreate", "Create a team", org, OpField.line("name", "Name"), OpField.line("join", "Join (everyone|invite)", "everyone")) { e, v ->
            e.client.orgs
                .teamCreate(org = v.v("org"), name = v.v("name"), join = if (v.v("join") == "invite") TeamJoin.INVITE else TeamJoin.EVERYONE)
                .teamId
        },
        op("getTeam", "Organizations", "orgs.teamGet", "A team", org, team) { e, v -> e.client.orgs.teamGet(org = v.v("org"), team = v.v("team")) },
        op("updateTeam", "Organizations", "orgs.teamUpdate", "Rename a team", org, team, OpField.line("name", "Name")) { e, v -> e.client.orgs.teamUpdate(org = v.v("org"), team = v.v("team"), name = v.opt("name")) },
        op("deleteTeam", "Organizations", "orgs.teamDelete", "Delete a team and its apps", org, team) { e, v -> e.client.orgs.teamDelete(org = v.v("org"), team = v.v("team")) },
        op("setTeamMember", "Organizations", "orgs.teamSetMember", "Add a member to a team", org, team, account, OpField.line("role", "Role (lead|member|readOnly)", "member")) { e, v ->
            e.client.orgs.teamSetMember(org = v.v("org"), team = v.v("team"), account = v.v("account"), role = TeamRole.decode(v.v("role")), passkey = e.passkey)
        },
        op("removeTeamMember", "Organizations", "orgs.teamRemoveMember", "Remove a member from a team", org, team, account) { e, v ->
            e.client.orgs.teamRemoveMember(org = v.v("org"), team = v.v("team"), account = v.v("account"), passkey = e.passkey)
        },
        op("upgradeRun", "Organizations", "orgs.upgradeStatus", "An organization-wide upgrade run", org, OpField.line("run", "Run id")) { e, v -> e.client.orgs.upgradeStatus(org = v.v("org"), run = v.v("run")) },
    )

private val approvalOps =
    listOf(
        op("pendingApprovals", "Approvals", "pendingApprovals", "What waits for a decision (blank: your own apps)", OpField.line("org", "Organization (optional)")) { e, v ->
            e.client.pendingApprovals(v.opt("org"))
        },
        op("appsReport", "Approvals", "orgs.apps", "One page of a scope's apps report", OpField.line("org", "Organization (optional)")) { e, v -> v.opt("org")?.let { e.client.orgs.apps(org = it) } ?: e.client.auth.accountApps() },
        op("releaseRules", "Approvals", "public.releasesPublic", "The cluster's governance and release deadlines") { e, _ -> e.client.public.releasesPublic() },
        op("requestConsent", "Approvals", "requestConsent", "Ask for a release approval on the node's consent page", OpField.line("version", "Release"), OpField.line("org", "Organization (optional)")) { e, v ->
            e.client.requestConsent(ConsentRequest.Release(ConsentScope(v.opt("org")), v.v("version"), SampleViewModel.CONSENT_CALLBACK))
        },
        op("consentCreateOrg", "Approvals", "requestConsent", "Ask to create an organization on the consent page", OpField.line("name", "Organization name")) { e, v ->
            e.client.requestConsent(ConsentRequest.CreateOrg(v.v("name"), SampleViewModel.CONSENT_CALLBACK))
        },
        op("consentCreateInvite", "Approvals", "requestConsent", "Ask to mint an invitation on the consent page", org, OpField.line("role", "Role (guest|member|admin)", "member")) { e, v ->
            e.client.requestConsent(ConsentRequest.CreateInvite(v.v("org"), checkNotNull(OrgRole.decode(v.v("role"))), SampleViewModel.CONSENT_CALLBACK))
        },
        op("consentAcceptInvite", "Approvals", "requestConsent", "Ask to accept an invitation on the consent page", OpField.line("token", "Invitation token")) { e, v ->
            e.client.requestConsent(ConsentRequest.AcceptInvite(v.v("token"), SampleViewModel.CONSENT_CALLBACK))
        },
        op("consentLeaveOrg", "Approvals", "requestConsent", "Ask to leave an organization on the consent page", org) { e, v ->
            e.client.requestConsent(ConsentRequest.LeaveOrg(v.v("org"), SampleViewModel.CONSENT_CALLBACK))
        },
        op("consentRemoveMember", "Approvals", "requestConsent", "Ask to remove a member on the consent page", org, account) { e, v ->
            e.client.requestConsent(ConsentRequest.RemoveMember(v.v("org"), v.v("account"), SampleViewModel.CONSENT_CALLBACK))
        },
        op("consentSetMemberRole", "Approvals", "requestConsent", "Ask to change a member's role on the consent page", org, account, OpField.line("role", "Role (guest|member|admin|owner)", "member")) { e, v ->
            e.client.requestConsent(ConsentRequest.SetMemberRole(v.v("org"), v.v("account"), checkNotNull(OrgRole.decode(v.v("role"))), SampleViewModel.CONSENT_CALLBACK))
        },
        op("consentSetMemberTeams", "Approvals", "requestConsent", "Ask to add a member to a team on the consent page", org, account, team, OpField.line("role", "Team role (lead|member|readOnly, optional)")) { e, v ->
            val membership = ConsentTeamMembership(v.v("team"), v.opt("role")?.let { checkNotNull(TeamRole.decode(it)) })
            e.client.requestConsent(ConsentRequest.SetMemberTeams(v.v("org"), v.v("account"), listOf(membership), SampleViewModel.CONSENT_CALLBACK))
        },
        op("consentRemoveMemberTeams", "Approvals", "requestConsent", "Ask to take a member out of a team on the consent page", org, account, team) { e, v ->
            e.client.requestConsent(ConsentRequest.RemoveMemberTeams(v.v("org"), v.v("account"), listOf(v.v("team")), SampleViewModel.CONSENT_CALLBACK))
        },
        op("consentRevokeInvite", "Approvals", "requestConsent", "Ask to revoke an invitation on the consent page", org, OpField.line("inviteKey", "Invitation key")) { e, v ->
            e.client.requestConsent(ConsentRequest.RevokeInvite(v.v("org"), v.v("inviteKey"), SampleViewModel.CONSENT_CALLBACK))
        },
        op("consentRenameOrg", "Approvals", "requestConsent", "Ask to rename an organization on the consent page", org, OpField.line("name", "New name")) { e, v ->
            e.client.requestConsent(ConsentRequest.RenameOrg(v.v("org"), v.v("name"), SampleViewModel.CONSENT_CALLBACK))
        },
        op("consentDeleteOrg", "Approvals", "requestConsent", "Ask to delete an organization on the consent page", org) { e, v ->
            e.client.requestConsent(ConsentRequest.DeleteOrg(v.v("org"), SampleViewModel.CONSENT_CALLBACK))
        },
        op("consentSetCapabilities", "Approvals", "requestConsent", "Ask to set capabilities on the consent page", org, OpField.line("capabilities", "Capability bits", "1"), OpField.line("account", "Account id (blank: a default)"), OpField.line("team", "Team id (optional)")) { e, v ->
            e.client.requestConsent(ConsentRequest.SetCapabilities(v.v("org"), v.v("capabilities").toLong(), SampleViewModel.CONSENT_CALLBACK, v.opt("account"), v.opt("team")))
        },
        op("consentSetAllApps", "Approvals", "requestConsent", "Ask to give a member every app on the consent page", org, account, OpField.line("allApps", "Every app (true|false)", "true")) { e, v ->
            e.client.requestConsent(ConsentRequest.SetAllApps(v.v("org"), v.v("account"), v.v("allApps").toBooleanStrict(), SampleViewModel.CONSENT_CALLBACK))
        },
        // The outcome prints with a created invitation's token redacted (ConsentOutcome.InviteCreated).
        op("consentResult", "Approvals", "consentResult", "The decision on a consent request", OpField.line("id", "Request id")) { e, v -> e.client.consentResult(v.v("id")) },
        op("resumeConsent", "Approvals", "resumeConsent", "The decision behind a link the consent page sent back", OpField.line("link", "Return link", "${SampleViewModel.CONSENT_CALLBACK}?request_id=…&result=approved"), OpField.line("id", "Request id you started")) { e, v ->
            e.client.resumeConsent(v.v("link"), v.opt("id")) ?: "not a consent return"
        },
        op("publisherBinding", "Approvals", "public.publisherBinding", "What a signing key signs to bind itself", OpField.line("signer", "Signer id")) { e, v -> e.client.public.publisherBinding(signer = v.v("signer")) },
    )

val sdkOperations: List<SDKOperation> = identityOps + appOps + packageOps + orgOps + approvalOps

val sdkCategories: List<String> = sdkOperations.map { it.category }.distinct()
