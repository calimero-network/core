@file:Suppress("MatchingDeclarationName")

package com.calimero.mero.sample.explorer

import android.content.Intent
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.Logout
import androidx.compose.material.icons.filled.Apps
import androidx.compose.material.icons.filled.Cloud
import androidx.compose.material.icons.filled.Forum
import androidx.compose.material.icons.outlined.AddCircleOutline
import androidx.compose.material.icons.outlined.Apps
import androidx.compose.material.icons.outlined.CheckCircle
import androidx.compose.material.icons.outlined.Clear
import androidx.compose.material.icons.outlined.ContentCopy
import androidx.compose.material.icons.outlined.DeleteSweep
import androidx.compose.material.icons.outlined.Dns
import androidx.compose.material.icons.outlined.ErrorOutline
import androidx.compose.material.icons.outlined.ExpandMore
import androidx.compose.material.icons.outlined.Fingerprint
import androidx.compose.material.icons.outlined.Forum
import androidx.compose.material.icons.outlined.Groups
import androidx.compose.material.icons.outlined.Home
import androidx.compose.material.icons.outlined.Info
import androidx.compose.material.icons.outlined.Link
import androidx.compose.material.icons.outlined.MonitorHeart
import androidx.compose.material.icons.outlined.PlayArrow
import androidx.compose.material.icons.outlined.Science
import androidx.compose.material.icons.outlined.Search
import androidx.compose.material.icons.outlined.Share
import androidx.compose.material.icons.outlined.VerifiedUser
import androidx.compose.material.icons.outlined.WarningAmber
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.NavigationBar
import androidx.compose.material3.NavigationBarItem
import androidx.compose.material3.NavigationBarItemDefaults
import androidx.compose.material3.Switch
import androidx.compose.material3.SwitchDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.calimero.mero.CalimeroClient
import com.calimero.mero.ConsentRequest
import com.calimero.mero.ConsentScope
import com.calimero.mero.PasskeyProvider
import com.calimero.mero.PendingApprovals
import com.calimero.mero.app.MeroAppBackend
import com.calimero.mero.app.OrgInvitation
import com.calimero.mero.compose.ConsentLauncher
import com.calimero.mero.sample.app.SampleLog
import com.calimero.mero.sample.app.SampleViewModel
import com.calimero.mero.sample.app.SignInMode
import com.calimero.mero.sample.chat.ChatScreen
import com.calimero.mero.sample.chat.ChatService
import com.calimero.mero.sample.mock.MockBackend
import com.calimero.mero.sample.ui.Cal
import com.calimero.mero.sample.ui.CalCard
import com.calimero.mero.sample.ui.CalIconTile
import com.calimero.mero.sample.ui.CalPrimaryButton
import com.calimero.mero.sample.ui.CalRow
import com.calimero.mero.sample.ui.CalSecondaryButton
import com.calimero.mero.sample.ui.CalSection
import com.calimero.mero.sample.ui.CalTextButton
import com.calimero.mero.sample.ui.CalTextField
import com.calimero.mero.sample.ui.Callout
import com.calimero.mero.sample.ui.CalloutKind
import com.calimero.mero.sample.ui.ChatAvatar
import com.calimero.mero.sample.ui.Hairline
import com.calimero.mero.sample.ui.IconTile
import com.calimero.mero.sample.ui.IdField
import com.calimero.mero.sample.ui.LimeGlow
import com.calimero.mero.sample.ui.LivePill
import com.calimero.mero.sample.ui.MeroExplorerTheme
import com.calimero.mero.sample.ui.RowDivider
import com.calimero.mero.sample.ui.SectionLabel
import com.calimero.mero.sample.ui.TechDetails
import com.calimero.mero.sample.ui.TopBar
import com.calimero.mero.sample.ui.screenPad
import kotlinx.coroutines.launch
import okhttp3.HttpUrl.Companion.toHttpUrlOrNull

/** Launch-time options for the explorer (e2e hooks). */
data class ExplorerOptions(
    val chatDisplayName: String? = null,
    val autoJoinInvite: String? = null,
    /** Prefills the developer node URL and opens the sign-in screen on it. */
    val nodeUrl: String? = null,
)

/** Root of the app: sign-in (a passkey on Calimero Cloud, or a development node), then the tabs. */
@Composable
fun ExplorerApp(
    vm: SampleViewModel,
    options: ExplorerOptions,
) {
    MeroExplorerTheme {
        val backend = vm.backend
        LaunchedEffect(backend?.account) {
            backend?.let { SampleLog.log(SampleLog.Level.OK, "signed in as account ${it.account.take(ACCOUNT_LOG)}…") }
        }
        when {
            vm.restoring -> Box(Modifier.fillMaxSize().background(Cal.bg), contentAlignment = Alignment.Center) { CircularProgressIndicator(color = Cal.accentInk) }
            backend == null -> LoginScreen(vm, options)
            else -> SignedIn(vm, backend, options)
        }
    }
}

// ---- Sign in -----------------------------------------------------------------------------

/**
 * Calimero Cloud first: the cluster's node URL (from the console) and the device's passkey,
 * through Android's Credential Manager. Under it, "Developer node" signs in to `merod local` with
 * mock attestation allowed and a software passkey kept on the device; debug builds also offer the
 * in-memory demo.
 */
@Composable
@Suppress("LongMethod", "CyclomaticComplexMethod")
private fun LoginScreen(
    vm: SampleViewModel,
    options: ExplorerOptions,
) {
    val context = LocalContext.current
    val focus = LocalFocusManager.current
    var dev by rememberSaveable { mutableStateOf(options.nodeUrl != null || vm.mode == SignInMode.DEV) }
    var cloudUrl by rememberSaveable { mutableStateOf(if (vm.mode == SignInMode.CLOUD) vm.nodeUrl else "") }
    var devUrl by rememberSaveable { mutableStateOf(options.nodeUrl ?: if (vm.mode == SignInMode.DEV) vm.nodeUrl else SampleViewModel.DEFAULT_DEV_NODE) }
    var name by rememberSaveable { mutableStateOf(options.chatDisplayName ?: "") }
    var newAccount by rememberSaveable { mutableStateOf(false) }
    var allowMock by rememberSaveable { mutableStateOf(true) }
    var localError by remember { mutableStateOf<String?>(null) }
    var showLogs by remember { mutableStateOf(false) }
    if (showLogs) {
        DiagnosticsScreen(onBack = { showLogs = false })
        return
    }
    val error = localError ?: vm.error

    fun valid(url: String): Boolean {
        val ok = url.trim().toHttpUrlOrNull() != null
        localError = if (ok) null else "Enter a full node URL, like ${SampleViewModel.DEFAULT_DEV_NODE}."
        return ok
    }

    Box(Modifier.fillMaxSize().background(Cal.bg)) {
        LimeGlow()
        Column(Modifier.fillMaxSize().statusBarsPadding().imePadding()) {
            Column(
                Modifier
                    .weight(1f)
                    .fillMaxWidth()
                    .verticalScroll(rememberScrollState())
                    .padding(horizontal = 24.dp),
                horizontalAlignment = Alignment.CenterHorizontally,
            ) {
                Spacer(Modifier.height(48.dp))
                Column(
                    Modifier.widthIn(max = 440.dp),
                    horizontalAlignment = Alignment.CenterHorizontally,
                    verticalArrangement = Arrangement.spacedBy(18.dp),
                ) {
                    CalIconTile(76.dp)
                    Column(horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(8.dp)) {
                        Text(
                            if (dev) "Connect to a dev node" else "Welcome to MeroKit",
                            color = Cal.text,
                            fontSize = 32.sp,
                            lineHeight = 38.sp,
                            fontWeight = FontWeight.Bold,
                            textAlign = TextAlign.Center,
                            modifier = Modifier.testTag("loginTitle"),
                        )
                        Text(
                            if (dev) {
                                "Sign in to merod local, or any --dev cluster. Its attestation is a mock, so allow it only for a node you run."
                            } else {
                                "Confidential apps on Calimero Cloud. The node proves what it runs before anything is sent, and you sign in with your passkey."
                            },
                            color = Cal.textDim,
                            fontSize = 16.sp,
                            lineHeight = 22.sp,
                            textAlign = TextAlign.Center,
                        )
                    }
                    Spacer(Modifier.height(4.dp))
                    if (error != null) Callout(error, kind = CalloutKind.DANGER, modifier = Modifier.testTag("loginError"))
                    CalCard(padding = 18.dp) {
                        Column(verticalArrangement = Arrangement.spacedBy(14.dp)) {
                            if (dev) {
                                CalTextField(
                                    devUrl,
                                    { devUrl = it },
                                    "Node URL",
                                    placeholder = SampleViewModel.DEFAULT_DEV_NODE,
                                    modifier = Modifier.testTag("nodeUrlField"),
                                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri, imeAction = ImeAction.Next),
                                )
                                CalTextField(name, { name = it }, "Your name", placeholder = "ada", modifier = Modifier.testTag("nameField"))
                                Row(verticalAlignment = Alignment.CenterVertically) {
                                    Column(Modifier.weight(1f)) {
                                        Text("Allow mock attestation", color = Cal.text, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
                                        Text("merod local signs no TDX quote", color = Cal.textFaint, fontSize = 12.sp)
                                    }
                                    Switch(
                                        checked = allowMock,
                                        onCheckedChange = { allowMock = it },
                                        colors = SwitchDefaults.colors(checkedTrackColor = Cal.lime, checkedThumbColor = Cal.onLime),
                                        modifier = Modifier.testTag("allowMock"),
                                    )
                                }
                                LoadingPrimaryButton(
                                    text = "Connect",
                                    icon = Icons.Outlined.Dns,
                                    loading = vm.busy,
                                    enabled = !vm.busy && devUrl.isNotBlank() && allowMock,
                                    onClick = {
                                        if (valid(devUrl)) {
                                            focus.clearFocus()
                                            vm.signInDev(devUrl, name)
                                        }
                                    },
                                    modifier = Modifier.testTag("loginButton"),
                                )
                                Text(
                                    "In the emulator, 10.0.2.2 is your computer. A software passkey is kept on this device for this node.",
                                    color = Cal.textFaint,
                                    fontSize = 12.sp,
                                    textAlign = TextAlign.Center,
                                    modifier = Modifier.fillMaxWidth(),
                                )
                            } else {
                                CalTextField(
                                    cloudUrl,
                                    { cloudUrl = it },
                                    "Cluster URL",
                                    placeholder = "https://<cluster>.calimero.network",
                                    modifier = Modifier.testTag("cloudUrlField"),
                                    keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri, imeAction = ImeAction.Done),
                                )
                                if (newAccount) {
                                    CalTextField(name, { name = it }, "Your name", placeholder = "ada", modifier = Modifier.testTag("nameField"))
                                }
                                LoadingPrimaryButton(
                                    text = if (newAccount) "Create account with a passkey" else "Sign in with passkey",
                                    icon = Icons.Outlined.Fingerprint,
                                    loading = vm.busy,
                                    enabled = !vm.busy && cloudUrl.isNotBlank() && (!newAccount || name.isNotBlank()),
                                    onClick = {
                                        if (valid(cloudUrl)) {
                                            focus.clearFocus()
                                            SampleLog.log(SampleLog.Level.REQ, "passkey ${if (newAccount) "sign-up" else "sign-in"} on $cloudUrl")
                                            vm.signInCloud(context, cloudUrl, newAccount, name)
                                        }
                                    },
                                    modifier = Modifier.testTag("cloudLoginButton"),
                                )
                                CalTextButton(
                                    if (newAccount) "I already have an account" else "New here? Create an account",
                                    onClick = { newAccount = !newAccount },
                                    underline = true,
                                    modifier = Modifier.align(Alignment.CenterHorizontally).testTag("toggleSignUp"),
                                )
                                Row(verticalAlignment = Alignment.CenterVertically) {
                                    Icon(Icons.Outlined.VerifiedUser, null, tint = Cal.textFaint, modifier = Modifier.size(14.dp))
                                    Spacer(Modifier.width(6.dp))
                                    Text("The node's TDX quote is verified before your passkey is used", color = Cal.textFaint, fontSize = 12.sp)
                                }
                            }
                        }
                    }
                    if (MockBackend.AVAILABLE) {
                        CalSecondaryButton(
                            "Try the in-app demo",
                            onClick = { vm.startDemo() },
                            icon = Icons.Outlined.Science,
                            modifier = Modifier.fillMaxWidth().testTag("demoButton"),
                        )
                    }
                    CalTextButton(
                        if (dev) "Use Calimero Cloud instead" else "Developer node (merod local)",
                        onClick = {
                            dev = !dev
                            localError = null
                        },
                        underline = !dev,
                        modifier = Modifier.testTag(if (dev) "useCloud" else "manualNode"),
                    )
                }
                Spacer(Modifier.height(24.dp))
            }
            CalTextButton(
                "Diagnostics",
                onClick = { showLogs = true },
                icon = Icons.Outlined.MonitorHeart,
                modifier = Modifier.align(Alignment.CenterHorizontally).padding(bottom = 16.dp),
            )
        }
    }
}

// ---- Signed in: Home, Chat, SDK -------------------------------------------------------------

@Composable
private fun SignedIn(
    vm: SampleViewModel,
    backend: MeroAppBackend,
    options: ExplorerOptions,
) {
    var tab by rememberSaveable { mutableStateOf(Tab.HOME) }
    var inChannel by remember { mutableStateOf(false) }
    var sdkOp by remember { mutableStateOf<SDKOperation?>(null) }
    var showLogs by remember { mutableStateOf(false) }
    val chat = remember(backend) { ChatService(backend, options.chatDisplayName, vm.revision) }
    val client = vm.client

    BackHandler(enabled = tab != Tab.HOME && !inChannel && sdkOp == null && !showLogs) { tab = Tab.HOME }

    if (showLogs) {
        DiagnosticsScreen(onBack = { showLogs = false })
        return
    }
    Column(Modifier.fillMaxSize().background(Cal.bg)) {
        Box(Modifier.weight(1f).fillMaxWidth()) {
            when (tab) {
                Tab.HOME ->
                    HomeScreen(
                        vm = vm,
                        backend = backend,
                        displayName = chat.username,
                        onOpenChat = { tab = Tab.CHAT },
                        onOpenSdk = { tab = Tab.SDK },
                        onOpenLogs = { showLogs = true },
                    )
                Tab.CHAT -> ChatScreen(chat, autoJoinInvite = options.autoJoinInvite, onInChannel = { inChannel = it })
                Tab.SDK -> {
                    val op = sdkOp
                    when {
                        client == null ->
                            Column(Modifier.fillMaxSize()) {
                                TopBar("Explore SDK")
                                Box(Modifier.padding(screenPad)) {
                                    Callout("The in-app demo has no cluster to call. Sign in to a node to explore the SDK.", kind = CalloutKind.INFO)
                                }
                            }
                        op == null -> SdkListScreen(onOpenOp = { sdkOp = it })
                        else -> {
                            BackHandler { sdkOp = null }
                            OperationRunner(client, op, vm.devPasskey, onBack = { sdkOp = null })
                        }
                    }
                }
            }
        }
        if (!(tab == Tab.CHAT && inChannel)) {
            Hairline()
            NavigationBar(containerColor = Cal.surface, tonalElevation = 0.dp) {
                Tab.entries.forEach { t ->
                    NavigationBarItem(
                        selected = tab == t,
                        onClick = { tab = t },
                        icon = { Icon(if (tab == t) t.selectedIcon else t.icon, contentDescription = null) },
                        label = { Text(t.label, fontSize = 12.sp, fontWeight = FontWeight.Medium) },
                        modifier = Modifier.testTag(t.tag),
                        colors =
                            NavigationBarItemDefaults.colors(
                                selectedIconColor = Cal.accentInk,
                                selectedTextColor = Cal.accentInk,
                                indicatorColor = Cal.accentSoft,
                                unselectedIconColor = Cal.textFaint,
                                unselectedTextColor = Cal.textFaint,
                            ),
                    )
                }
            }
        }
    }
}

@Composable
@Suppress("LongMethod", "LongParameterList")
private fun HomeScreen(
    vm: SampleViewModel,
    backend: MeroAppBackend,
    displayName: String,
    onOpenChat: () -> Unit,
    onOpenSdk: () -> Unit,
    onOpenLogs: () -> Unit,
) {
    Box(Modifier.fillMaxSize().background(Cal.bg)) {
        LimeGlow(height = 320.dp)
        Column(
            Modifier
                .fillMaxSize()
                .statusBarsPadding()
                .verticalScroll(rememberScrollState())
                .padding(horizontal = screenPad + 4.dp, vertical = 12.dp),
            verticalArrangement = Arrangement.spacedBy(22.dp),
        ) {
            Row(Modifier.padding(top = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                ChatAvatar(displayName.ifEmpty { "?" }, size = 52.dp)
                Spacer(Modifier.width(14.dp))
                Column(Modifier.weight(1f)) {
                    Text("Signed in as", color = Cal.textFaint, fontSize = 13.sp)
                    Text(
                        displayName,
                        color = Cal.text,
                        fontSize = 24.sp,
                        fontWeight = FontWeight.Bold,
                        maxLines = 1,
                        modifier = Modifier.testTag("homeTitle"),
                    )
                }
                CalIconTile(34.dp)
            }
            ConnectionCard(vm, backend)
            OrganizationsSection(vm, backend)
            vm.client?.let { ApprovalsSection(it) }
            CalSection("Try it") {
                CalRow(
                    Icons.Filled.Forum,
                    "Chat",
                    onClick = onOpenChat,
                    subtitle = "Spaces, channels and live messages",
                    accent = true,
                    modifier = Modifier.testTag("openChat"),
                )
                RowDivider()
                CalRow(
                    Icons.Filled.Apps,
                    "Explore SDK",
                    onClick = onOpenSdk,
                    subtitle = "${sdkOperations.size} methods in ${sdkCategories.size} categories",
                    modifier = Modifier.testTag("exploreSDK"),
                )
            }
            CalSection("Session") {
                CalRow(Icons.Outlined.MonitorHeart, "Diagnostics", onClick = onOpenLogs, subtitle = "Every request and event")
                RowDivider()
                Row(
                    Modifier
                        .fillMaxWidth()
                        .clickable {
                            SampleLog.log(SampleLog.Level.REQ, "sign out")
                            vm.signOut()
                        }.padding(horizontal = 14.dp, vertical = 12.dp)
                        .testTag("signOutButton"),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Box(
                        Modifier.size(36.dp).clip(RoundedCornerShape(11.dp)).background(Cal.errorSoft),
                        contentAlignment = Alignment.Center,
                    ) {
                        Icon(Icons.AutoMirrored.Outlined.Logout, contentDescription = null, tint = Cal.error, modifier = Modifier.size(18.dp))
                    }
                    Spacer(Modifier.width(12.dp))
                    Text("Sign out", color = Cal.error, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
                }
            }
            Spacer(Modifier.height(8.dp))
        }
    }
}

@Composable
private fun ConnectionCard(
    vm: SampleViewModel,
    backend: MeroAppBackend,
) {
    val demo = vm.mode == SignInMode.DEMO
    val node = vm.client?.nodeUrl
    val status =
        when {
            demo -> "In-app demo, no network"
            else -> node?.toHttpUrlOrNull()?.let { if (it.port == 443 || it.port == 80) it.host else "${it.host}:${it.port}" } ?: node.orEmpty()
        }
    CalCard(Modifier.testTag("homeAccount"), padding = 16.dp) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            IconTile(if (vm.mode == SignInMode.CLOUD) Icons.Filled.Cloud else Icons.Outlined.Dns, accent = true, size = 40.dp)
            Spacer(Modifier.width(12.dp))
            Column(Modifier.weight(1f)) {
                Text(
                    when (vm.mode) {
                        SignInMode.CLOUD -> "Calimero Cloud"
                        SignInMode.DEV -> "Development node"
                        SignInMode.DEMO -> "Demo"
                    },
                    color = Cal.text,
                    fontSize = 15.sp,
                    fontWeight = FontWeight.SemiBold,
                )
                Text(status, color = Cal.textFaint, fontSize = 13.sp, maxLines = 1)
            }
            LivePill(true)
        }
        TechDetails {
            IdField("Account", backend.account)
            node?.let { IdField("Node", it) }
            if (vm.mode == SignInMode.DEV) IdField("Attestation", "mock (allowed for this node)")
        }
    }
}

/** The account's organizations and their teams: create one, invite to one, redeem an invite. */
@Composable
@Suppress("LongMethod")
private fun OrganizationsSection(
    vm: SampleViewModel,
    backend: MeroAppBackend,
) {
    val scope = rememberCoroutineScope()
    val clipboard = LocalClipboardManager.current
    val revision by vm.revision.collectAsState()
    var orgs by remember { mutableStateOf<List<Pair<String, String>>>(emptyList()) }
    var teams by remember { mutableStateOf<Map<String, List<String>>>(emptyMap()) }
    var note by remember { mutableStateOf<String?>(null) }
    var newOrg by remember { mutableStateOf("") }
    var invite by remember { mutableStateOf("") }

    suspend fun reload() {
        runCatching {
            val listed = backend.listNamespaces()
            orgs = listed.map { it.id to (it.name ?: "Organization") }
            teams = listed.associate { org -> org.id to backend.listSubgroups(org.id).map { it.name ?: "team" } }
        }.onFailure { note = "Couldn't list organizations: ${it.message}" }
    }
    LaunchedEffect(backend, revision) { reload() }

    CalSection("Organizations") {
        if (orgs.isEmpty()) {
            Text(
                "No organizations yet. Create one, or redeem an invite.",
                color = Cal.textFaint,
                fontSize = 13.sp,
                modifier = Modifier.padding(14.dp).testTag("noOrgs"),
            )
        }
        orgs.forEachIndexed { i, (id, name) ->
            if (i > 0) RowDivider()
            CalRow(
                Icons.Outlined.Groups,
                name,
                onClick = {
                    scope.launch {
                        runCatching { backend.createInvitation(id).copy(name = name) }
                            .onSuccess {
                                clipboard.setText(AnnotatedString(it.link(ChatService.PACKAGE_NAME)))
                                note = "Invite link to $name copied."
                            }.onFailure { note = "Couldn't create an invite: ${it.message}" }
                    }
                },
                subtitle = teams[id].orEmpty().let { if (it.isEmpty()) "No teams yet · tap to copy an invite" else it.joinToString(" · ") + " · tap to copy an invite" },
                modifier = Modifier.testTag("org:$name"),
            )
        }
    }
    note?.let { Callout(it, kind = CalloutKind.INFO, modifier = Modifier.testTag("orgNote")) }
    CalCard(padding = 16.dp) {
        Column(verticalArrangement = Arrangement.spacedBy(10.dp)) {
            CalTextField(newOrg, { newOrg = it }, "New organization", placeholder = "Acme", modifier = Modifier.testTag("newOrgField"))
            CalSecondaryButton(
                "Create with a general team",
                icon = Icons.Outlined.AddCircleOutline,
                enabled = newOrg.isNotBlank(),
                onClick = {
                    val name = newOrg.trim()
                    scope.launch {
                        runCatching {
                            val org = backend.foundNamespace(name).namespaceId
                            backend.createSubgroup(org, "general", isPublic = true)
                        }.onSuccess {
                            newOrg = ""
                            note = "Created $name."
                            reload()
                        }.onFailure { note = "Couldn't create $name: ${it.message}" }
                    }
                },
                modifier = Modifier.fillMaxWidth().testTag("createOrg"),
            )
            CalTextField(invite, { invite = it }, "Invite link or code", placeholder = "https://links.calimero.network/…", modifier = Modifier.testTag("inviteField"))
            CalSecondaryButton(
                "Redeem invite",
                icon = Icons.Outlined.Link,
                enabled = invite.isNotBlank(),
                onClick = {
                    val parsed = OrgInvitation.parse(invite)
                    if (parsed == null) {
                        note = "That isn't an invite."
                    } else {
                        scope.launch {
                            runCatching { backend.joinNamespace(parsed.namespaceId, parsed) }
                                .onSuccess {
                                    invite = ""
                                    note = "Joined ${parsed.name ?: "the organization"}."
                                    reload()
                                }.onFailure { note = "Couldn't redeem it: ${it.message}" }
                        }
                    }
                },
                modifier = Modifier.fillMaxWidth().testTag("redeemInvite"),
            )
        }
    }
}

/**
 * What waits for the account holder's decision: mero-tee releases the cluster runs that their
 * scope has not approved, and app upgrades. Approving opens the node's consent page in a Custom Tab.
 */
@Composable
private fun ApprovalsSection(client: CalimeroClient) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var pending by remember { mutableStateOf<PendingApprovals?>(null) }
    var note by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(client) { pending = runCatching { client.pendingApprovals() }.onFailure { note = it.message }.getOrNull() }
    val releases = pending?.releases.orEmpty()
    CalSection("Approvals") {
        if (releases.isEmpty()) {
            Text(
                if (pending == null) "Checking…" else "Nothing waits for your approval.",
                color = Cal.textFaint,
                fontSize = 13.sp,
                modifier = Modifier.padding(14.dp).testTag("noApprovals"),
            )
        }
        releases.forEachIndexed { i, release ->
            if (i > 0) RowDivider()
            CalRow(
                Icons.Outlined.VerifiedUser,
                "Node release ${release.version}",
                onClick = {
                    scope.launch {
                        runCatching {
                            ConsentLauncher.request(context, client, ConsentRequest.Release(ConsentScope.Account, release.version, SampleViewModel.CONSENT_CALLBACK))
                        }.onSuccess { started ->
                            if (started.code.isNotEmpty()) note = "Check the page shows the code ${started.code} before you approve."
                        }.onFailure { note = "Couldn't open the consent page: ${it.message}" }
                    }
                },
                subtitle = "${release.nodes} node(s) · review on the consent page",
            )
        }
    }
    note?.let { Callout(it, kind = CalloutKind.WARNING) }
}

/** The lime primary button, full width, with a spinner in place of the icon while [loading]. */
@Composable
private fun LoadingPrimaryButton(
    text: String,
    icon: ImageVector,
    loading: Boolean,
    enabled: Boolean,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
) {
    Box(contentAlignment = Alignment.Center) {
        CalPrimaryButton(text, onClick = onClick, enabled = enabled, icon = if (loading) null else icon, modifier = modifier.fillMaxWidth())
        if (loading) {
            CircularProgressIndicator(
                color = Cal.onLime,
                strokeWidth = 2.dp,
                modifier = Modifier.align(Alignment.CenterStart).padding(start = 20.dp).size(18.dp),
            )
        }
    }
}

private enum class Tab(
    val label: String,
    val icon: ImageVector,
    val selectedIcon: ImageVector,
    val tag: String,
) {
    HOME("Home", Icons.Outlined.Home, Icons.Outlined.Home, "tabHome"),
    CHAT("Chat", Icons.Outlined.Forum, Icons.Filled.Forum, "tabChat"),
    SDK("SDK", Icons.Outlined.Apps, Icons.Filled.Apps, "tabSdk"),
}

// ---- Diagnostics ---------------------------------------------------------------------------

@Composable
private fun DiagnosticsScreen(onBack: () -> Unit) {
    val context = LocalContext.current
    val clipboard = LocalClipboardManager.current
    val listState = rememberLazyListState()
    BackHandler(onBack = onBack)
    LaunchedEffect(SampleLog.lines.size) {
        if (SampleLog.lines.isNotEmpty()) listState.animateScrollToItem(SampleLog.lines.size - 1)
    }
    Column(Modifier.fillMaxSize().background(Cal.bg)) {
        TopBar(
            "Diagnostics",
            onBack = onBack,
            actions = {
                IconButton(onClick = SampleLog::clear) { Icon(Icons.Outlined.DeleteSweep, "Clear", tint = Cal.accentInk) }
                IconButton(onClick = { clipboard.setText(AnnotatedString(SampleLog.text())) }) {
                    Icon(Icons.Outlined.ContentCopy, "Copy", tint = Cal.accentInk)
                }
                IconButton(onClick = {
                    val send = Intent(Intent.ACTION_SEND).setType("text/plain").putExtra(Intent.EXTRA_TEXT, SampleLog.text())
                    context.startActivity(Intent.createChooser(send, "Share diagnostics"))
                }) { Icon(Icons.Outlined.Share, "Share", tint = Cal.accentInk) }
            },
        )
        LazyColumn(
            state = listState,
            modifier = Modifier.fillMaxSize().padding(horizontal = 16.dp),
            verticalArrangement = Arrangement.spacedBy(5.dp),
        ) {
            item { Spacer(Modifier.height(10.dp)) }
            if (SampleLog.lines.isEmpty()) item { Text("No activity yet.", color = Cal.textDim, fontSize = 13.sp) }
            items(SampleLog.lines, key = { it.id }) { line ->
                Row(verticalAlignment = Alignment.Top) {
                    val (icon, tint) =
                        when (line.level) {
                            SampleLog.Level.ERR -> Icons.Outlined.ErrorOutline to Cal.error
                            SampleLog.Level.OK -> Icons.Outlined.CheckCircle to Cal.success
                            SampleLog.Level.WARN -> Icons.Outlined.WarningAmber to Cal.warning
                            SampleLog.Level.REQ -> Icons.Outlined.PlayArrow to Cal.text
                            SampleLog.Level.INFO -> Icons.Outlined.Info to Cal.textDim
                        }
                    Icon(icon, null, tint = tint, modifier = Modifier.padding(top = 2.dp).size(13.dp))
                    Spacer(Modifier.width(8.dp))
                    Text(line.text, color = Cal.text, fontSize = 12.sp, fontFamily = FontFamily.Monospace)
                }
            }
            item { Spacer(Modifier.height(16.dp)) }
        }
    }
}

/** The categorized, searchable SDK surface. */
@Composable
@Suppress("LongMethod")
private fun SdkListScreen(
    onOpenOp: (SDKOperation) -> Unit,
    onBack: (() -> Unit)? = null,
) {
    var search by remember { mutableStateOf("") }
    val expanded = remember { mutableStateMapOf<String, Boolean>() }
    val query = search.trim().lowercase()
    val sections =
        sdkCategories.mapNotNull { category ->
            val ops =
                sdkOperations.filter { op ->
                    op.category == category &&
                        (query.isEmpty() || "${op.name} ${op.summary} ${op.category}".lowercase().contains(query))
                }
            if (ops.isEmpty()) null else category to ops
        }

    Column(Modifier.fillMaxSize().background(Cal.bg)) {
        TopBar("Explore SDK", subtitle = "${sdkOperations.size} methods in ${sdkCategories.size} categories", onBack = onBack)
        LazyColumn(
            Modifier.fillMaxSize().padding(horizontal = screenPad).testTag("opList"),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            item {
                Row(
                    Modifier
                        .padding(top = 16.dp)
                        .fillMaxWidth()
                        .clip(RoundedCornerShape(Cal.radiusControl))
                        .background(Cal.surface)
                        .border(1.dp, Cal.borderStrong, RoundedCornerShape(Cal.radiusControl))
                        .padding(start = 12.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Icon(Icons.Outlined.Search, null, tint = Cal.textFaint, modifier = Modifier.size(18.dp))
                    androidx.compose.foundation.text.BasicTextField(
                        value = search,
                        onValueChange = { search = it },
                        singleLine = true,
                        textStyle =
                            androidx.compose.ui.text
                                .TextStyle(color = Cal.text, fontSize = 14.sp),
                        modifier = Modifier.weight(1f).padding(horizontal = 10.dp, vertical = 12.dp).testTag("sdkSearch"),
                        decorationBox = { inner ->
                            if (search.isEmpty()) Text("Search methods", color = Cal.textFaint, fontSize = 14.sp)
                            inner()
                        },
                    )
                    if (search.isNotEmpty()) {
                        IconButton(onClick = { search = "" }, modifier = Modifier.testTag("sdkSearchClear")) {
                            Icon(Icons.Outlined.Clear, "Clear search", tint = Cal.textFaint)
                        }
                    }
                }
            }
            items(sections, key = { it.first }) { (category, ops) ->
                val open = search.isNotEmpty() || expanded[category] == true
                CalCard(padding = 0.dp) {
                    Row(
                        Modifier
                            .fillMaxWidth()
                            .clickable { expanded[category] = !(expanded[category] ?: false) }
                            .padding(horizontal = 20.dp, vertical = 14.dp)
                            .testTag("category:$category"),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        Text(category, color = Cal.text, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f))
                        Text("${ops.size}", color = Cal.textFaint, fontSize = 12.5.sp)
                        Icon(
                            Icons.Outlined.ExpandMore,
                            null,
                            tint = Cal.textFaint,
                            modifier = Modifier.padding(start = 6.dp).size(18.dp).rotate(if (open) 180f else 0f),
                        )
                    }
                    if (open) {
                        ops.forEach { op ->
                            Hairline()
                            Column(
                                Modifier
                                    .fillMaxWidth()
                                    .clickable { onOpenOp(op) }
                                    .padding(horizontal = 20.dp, vertical = 12.dp)
                                    .testTag("op:${op.name}"),
                            ) {
                                Text(op.name, color = Cal.text, fontSize = 14.sp, fontWeight = FontWeight.Medium, fontFamily = FontFamily.Monospace)
                                Text(op.summary, color = Cal.textFaint, fontSize = 12.5.sp)
                            }
                        }
                    }
                }
            }
            item { Spacer(Modifier.size(16.dp)) }
        }
    }
}

@Composable
private fun OperationRunner(
    client: CalimeroClient,
    op: SDKOperation,
    devPasskey: PasskeyProvider?,
    onBack: () -> Unit,
) {
    val scope = rememberCoroutineScope()
    val context = LocalContext.current
    val inputs = remember { mutableStateMapOf<String, String>() }
    var output by remember { mutableStateOf("") }
    var failed by remember { mutableStateOf(false) }
    var running by remember { mutableStateOf(false) }

    Column(Modifier.fillMaxSize().background(Cal.bg)) {
        TopBar(op.name, subtitle = op.category, onBack = onBack)
        LazyColumn(
            Modifier.fillMaxSize().padding(horizontal = screenPad),
            verticalArrangement = Arrangement.spacedBy(16.dp),
        ) {
            item { Text(op.summary, color = Cal.textDim, fontSize = 14.sp, modifier = Modifier.padding(top = 16.dp)) }
            items(op.fields, key = { it.id }) { field ->
                CalTextField(
                    value = inputs[field.id] ?: "",
                    onValueChange = { inputs[field.id] = it },
                    label = field.label,
                    placeholder = field.placeholder.ifEmpty { null },
                    singleLine = field.kind == OpFieldKind.LINE,
                    mono = true,
                )
            }
            item {
                CalPrimaryButton(
                    text = if (running) "Running…" else "Run",
                    icon = Icons.Outlined.PlayArrow,
                    enabled = !running,
                    modifier = Modifier.fillMaxWidth().testTag("runOp"),
                    onClick = {
                        running = true
                        scope.launch {
                            try {
                                output = op.run(OpEnv(client, context, devPasskey), inputs.toMap())
                                failed = false
                            } catch (e: Exception) {
                                output = e.message ?: e.toString()
                                failed = true
                            } finally {
                                running = false
                            }
                        }
                    },
                )
            }
            if (output.isNotEmpty()) {
                item {
                    Column {
                        SectionLabel(if (failed) "Error" else "Response")
                        Spacer(Modifier.size(6.dp))
                        Row(
                            Modifier
                                .fillMaxWidth()
                                .clip(RoundedCornerShape(10.dp))
                                .background(if (failed) Cal.errorSoft else Cal.surfaceSunken)
                                .border(1.dp, Cal.border, RoundedCornerShape(10.dp))
                                .horizontalScroll(rememberScrollState())
                                .padding(12.dp),
                        ) {
                            Text(
                                output,
                                color = if (failed) Cal.error else Cal.text,
                                fontFamily = FontFamily.Monospace,
                                fontSize = 12.sp,
                                modifier = Modifier.testTag("opResponse"),
                            )
                        }
                    }
                }
            }
            item { Spacer(Modifier.size(16.dp)) }
        }
    }
}

private const val ACCOUNT_LOG = 12
