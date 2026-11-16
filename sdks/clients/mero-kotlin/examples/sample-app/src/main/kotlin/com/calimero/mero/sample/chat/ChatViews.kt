@file:Suppress("TooManyFunctions")

package com.calimero.mero.sample.chat

import android.content.Intent
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.ArrowBack
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.ArrowUpward
import androidx.compose.material.icons.filled.Download
import androidx.compose.material.icons.filled.Forum
import androidx.compose.material.icons.filled.Person
import androidx.compose.material.icons.filled.Tag
import androidx.compose.material.icons.outlined.AddComment
import androidx.compose.material.icons.outlined.CheckCircle
import androidx.compose.material.icons.outlined.ChevronRight
import androidx.compose.material.icons.outlined.ConfirmationNumber
import androidx.compose.material.icons.outlined.ContentCopy
import androidx.compose.material.icons.outlined.ContentPaste
import androidx.compose.material.icons.outlined.ErrorOutline
import androidx.compose.material.icons.outlined.Forum
import androidx.compose.material.icons.outlined.PersonAddAlt
import androidx.compose.material.icons.outlined.Refresh
import androidx.compose.material.icons.outlined.Share
import androidx.compose.material.icons.outlined.Sync
import androidx.compose.material.icons.outlined.Tag
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.OutlinedTextFieldDefaults
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.window.Dialog
import com.calimero.mero.sample.ui.Cal
import com.calimero.mero.sample.ui.CalPrimaryButton
import com.calimero.mero.sample.ui.CalSecondaryButton
import com.calimero.mero.sample.ui.CalSection
import com.calimero.mero.sample.ui.Callout
import com.calimero.mero.sample.ui.CalloutKind
import com.calimero.mero.sample.ui.ChatAvatar
import com.calimero.mero.sample.ui.EmptyState
import com.calimero.mero.sample.ui.IconTile
import com.calimero.mero.sample.ui.RowDivider
import com.calimero.mero.sample.ui.SpaceAvatar
import com.calimero.mero.sample.ui.TopBar
import com.calimero.mero.sample.ui.screenPad
import kotlinx.coroutines.launch
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

/**
 * The Chat tab: spaces, then a space's channels, then a channel's messages. [onInChannel] says
 * when a channel is open, so the app can hide its bottom bar there.
 *
 * [autoJoinInvite] is the e2e hook (the `invite` launch extra, debug builds only): when set, the screen installs
 * mero-chat if needed and redeems that invite on open.
 */
@Composable
fun ChatScreen(
    service: ChatService,
    autoJoinInvite: String? = null,
    onInChannel: (Boolean) -> Unit = {},
) {
    LaunchedEffect(Unit) {
        val invite = autoJoinInvite?.takeIf { it.isNotEmpty() && service.claimAutoJoin() }
        if (invite != null) {
            if (service.appId == null) service.setup()
            service.joinSpace(invite)
        } else {
            // Skip the install gate if mero-chat is already installed on this node.
            service.detectInstalled()
        }
    }
    LaunchedEffect(service.openChannel) { onInChannel(service.openChannel != null) }

    val openSpace = service.openSpace
    val openChannel = service.openChannel
    when {
        openSpace == null -> ChatHome(service, onOpen = { service.openSpace = it })
        openChannel == null -> {
            BackHandler { service.openSpace = null }
            ChannelsScreen(service, openSpace, onBack = { service.openSpace = null }, onOpen = { service.openChannel = it })
        }
        else -> {
            BackHandler { service.openChannel = null }
            ChannelScreen(service, openChannel, onBack = { service.openChannel = null })
        }
    }
}

// ---- Chat home (spaces) ----------------------------------------------------------------------

@Composable
@Suppress("LongMethod")
private fun ChatHome(
    service: ChatService,
    onOpen: (ChatSpace) -> Unit,
) {
    val scope = rememberCoroutineScope()
    var newSpace by remember { mutableStateOf(false) }
    var joining by remember { mutableStateOf(false) }
    // Re-list whenever the session changes (a sign-in, a redeemed invite).
    val revision by service.revision.collectAsState()
    LaunchedEffect(revision) { service.loadSpaces() }

    Box(Modifier.fillMaxSize().background(Cal.bg)) {
        Column(Modifier.fillMaxSize()) {
            LargeTitleBar(
                "Chat",
                actions = {
                    if (service.appId != null) {
                        AddMenu(tag = "chatAdd") { close ->
                            if (service.offerNewSpace) {
                                MenuItem("New space", Icons.Outlined.AddComment) {
                                    close()
                                    newSpace = true
                                }
                            }
                            MenuItem("Join with an invite", Icons.Outlined.ConfirmationNumber) {
                                close()
                                joining = true
                            }
                            HorizontalDivider(color = Cal.border)
                            MenuItem("Refresh", Icons.Outlined.Refresh) {
                                close()
                                scope.launch { service.loadSpaces() }
                            }
                        }
                    }
                },
            )
            if (service.appId == null) {
                InstallGate(service)
            } else {
                Column(
                    Modifier
                        .fillMaxSize()
                        .verticalScroll(rememberScrollState())
                        .padding(horizontal = screenPad + 4.dp, vertical = 8.dp),
                    verticalArrangement = Arrangement.spacedBy(14.dp),
                ) {
                    StatusLine(service)
                    if (service.spaces.isEmpty() && service.spacesLoad == SpacesLoad.PENDING) {
                        EmptyState(Icons.Outlined.Forum, "Loading spaces…", "Waiting for your session to connect.")
                    } else if (service.spaces.isEmpty() && service.spacesLoad == SpacesLoad.FAILED) {
                        // A failed listing is not an empty one: no "create" here, only a retry.
                        EmptyState(
                            Icons.Outlined.Forum,
                            "Couldn't load your spaces",
                            "Your spaces may still be there. Try again once the connection is back.",
                        ) {
                            CalSecondaryButton(
                                "Try again",
                                onClick = { scope.launch { service.loadSpaces() } },
                                modifier = Modifier.testTag("chatRetrySpaces"),
                                icon = Icons.Outlined.Refresh,
                            )
                        }
                    } else if (service.spaces.isEmpty()) {
                        EmptyState(
                            Icons.Outlined.Forum,
                            "No spaces yet",
                            if (service.canCreateSpaces) {
                                "Create a space with +, or join one with an invite."
                            } else {
                                "Join a space with an invite from its admin. Tap + to paste one."
                            },
                        )
                    } else {
                        CalSection("Spaces") {
                            service.spaces.forEachIndexed { i, s ->
                                Row(
                                    Modifier
                                        .fillMaxWidth()
                                        .clickable { onOpen(s) }
                                        .heightIn(min = 66.dp)
                                        .padding(horizontal = 14.dp, vertical = 10.dp),
                                    verticalAlignment = Alignment.CenterVertically,
                                ) {
                                    SpaceAvatar(s.name)
                                    Spacer(Modifier.width(12.dp))
                                    Column(Modifier.weight(1f)) {
                                        Text(s.name, color = Cal.text, fontSize = 16.sp, fontWeight = FontWeight.SemiBold)
                                        Text("Space", color = Cal.textFaint, fontSize = 13.sp)
                                    }
                                    Icon(Icons.Outlined.ChevronRight, null, tint = Cal.textFaint, modifier = Modifier.size(18.dp))
                                }
                                if (i < service.spaces.size - 1) RowDivider()
                            }
                        }
                    }
                    Spacer(Modifier.size(8.dp))
                }
            }
        }
        if (service.busy) BusyOverlay(service.status)
    }

    if (newSpace) {
        NameDialog(
            title = "New space",
            message = "A space holds channels and the people you invite.",
            placeholder = "Space name",
            onDismiss = { newSpace = false },
            onCreate = { name ->
                newSpace = false
                scope.launch { service.createSpace(name) }
            },
        )
    }
    if (joining) JoinDialog(service, onDismiss = { joining = false })
}

@Composable
private fun InstallGate(service: ChatService) {
    val scope = rememberCoroutineScope()
    Column(
        Modifier
            .fillMaxSize()
            .verticalScroll(rememberScrollState())
            .padding(horizontal = 28.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(22.dp),
    ) {
        Spacer(Modifier.size(28.dp))
        Box(Modifier.size(112.dp).clip(CircleShape).background(Cal.accentSoft), contentAlignment = Alignment.Center) {
            Icon(Icons.Filled.Forum, null, tint = Cal.accentInk, modifier = Modifier.size(48.dp))
        }
        Column(horizontalAlignment = Alignment.CenterHorizontally, verticalArrangement = Arrangement.spacedBy(8.dp)) {
            Text("Set up Mero Chat", color = Cal.text, fontSize = 24.sp, fontWeight = FontWeight.Bold)
            Text(
                "Installs Mero Chat (com.calimero.chat) from the Calimero app registry onto your node. " +
                    "Messages live on your node, not on a server.",
                color = Cal.textDim,
                fontSize = 16.sp,
                lineHeight = 22.sp,
                textAlign = TextAlign.Center,
            )
        }
        Box(Modifier.widthIn(max = 420.dp).fillMaxWidth(), contentAlignment = Alignment.Center) {
            CalPrimaryButton(
                "Install Mero Chat",
                onClick = { scope.launch { service.setup() } },
                enabled = !service.busy,
                icon = if (service.busy) null else Icons.Filled.Download,
                modifier = Modifier.fillMaxWidth().testTag("installChat"),
            )
            if (service.busy) {
                CircularProgressIndicator(
                    color = Cal.onLime,
                    strokeWidth = 2.dp,
                    modifier = Modifier.align(Alignment.CenterStart).padding(start = 20.dp).size(18.dp),
                )
            }
        }
        StatusLine(service)
    }
}

// ---- Channels in a space ---------------------------------------------------------------------

@Composable
@Suppress("LongMethod")
private fun ChannelsScreen(
    service: ChatService,
    space: ChatSpace,
    onBack: () -> Unit,
    onOpen: (ChatChannel) -> Unit,
) {
    val scope = rememberCoroutineScope()
    var newChannel by remember { mutableStateOf(false) }
    var invite by remember { mutableStateOf<String?>(null) }
    var inviteError by remember { mutableStateOf(false) }
    LaunchedEffect(space.id) { service.loadChannels(space) }

    Column(Modifier.fillMaxSize().background(Cal.bg)) {
        LargeTitleBar(
            space.name,
            onBack = onBack,
            actions = {
                AddMenu(tag = "channelAdd") { close ->
                    MenuItem("New channel", Icons.Outlined.Tag) {
                        close()
                        newChannel = true
                    }
                    if (service.canInvite) {
                        MenuItem("Invite people", Icons.Outlined.PersonAddAlt) {
                            close()
                            scope.launch {
                                val code = service.makeInvite(space)
                                if (code != null) invite = code else inviteError = true
                            }
                        }
                    }
                    MenuItem("Sync now", Icons.Outlined.Sync) {
                        close()
                        scope.launch { service.resync(space) }
                    }
                }
            },
        )
        Column(
            Modifier
                .fillMaxSize()
                .verticalScroll(rememberScrollState())
                .padding(horizontal = screenPad + 4.dp, vertical = 12.dp),
            verticalArrangement = Arrangement.spacedBy(14.dp),
        ) {
            if (service.status.isNotEmpty()) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    if (service.busy) {
                        CircularProgressIndicator(color = Cal.accentInk, strokeWidth = 2.dp, modifier = Modifier.size(14.dp))
                        Spacer(Modifier.width(8.dp))
                    }
                    Text(service.status, color = if (service.statusIsError) Cal.error else Cal.textFaint, fontSize = 13.sp)
                }
            }
            if (service.channels.isEmpty() && !service.busy) {
                EmptyState(
                    Icons.Outlined.Tag,
                    "No channels yet",
                    "If you just joined, channels sync from the inviter. Or create one with +.",
                )
                CalSecondaryButton(
                    "Sync now",
                    onClick = { scope.launch { service.resync(space) } },
                    icon = Icons.Outlined.Sync,
                    modifier = Modifier.fillMaxWidth(),
                )
            } else if (service.channels.isNotEmpty()) {
                CalSection("Channels") {
                    service.channels.forEachIndexed { i, ch ->
                        Row(
                            Modifier
                                .fillMaxWidth()
                                .clickable { onOpen(ch) }
                                .heightIn(min = 60.dp)
                                .padding(horizontal = 14.dp, vertical = 10.dp),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            IconTile(if (ch.kind == "Dm") Icons.Filled.Person else Icons.Filled.Tag, accent = true, size = 38.dp)
                            Spacer(Modifier.width(12.dp))
                            Text(ch.name, color = Cal.text, fontSize = 16.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f))
                            Icon(Icons.Outlined.ChevronRight, null, tint = Cal.textFaint, modifier = Modifier.size(18.dp))
                        }
                        if (i < service.channels.size - 1) RowDivider()
                    }
                }
            }
        }
    }

    if (newChannel) {
        NameDialog(
            title = "New channel",
            message = null,
            placeholder = "channel-name",
            onDismiss = { newChannel = false },
            onCreate = { name ->
                newChannel = false
                scope.launch { service.createChannel(space, name) }
            },
        )
    }
    invite?.let { code -> InviteDialog(code, space.name, onDismiss = { invite = null }) }
    if (inviteError) {
        AlertDialog(
            onDismissRequest = { inviteError = false },
            containerColor = Cal.surface,
            title = { Text("Couldn't create invite", color = Cal.text) },
            text = { Text(service.status.ifEmpty { "The node did not return an invitation." }, color = Cal.textDim) },
            confirmButton = { TextButton(onClick = { inviteError = false }) { Text("OK", color = Cal.accentInk) } },
        )
    }
}

// ---- A channel's messages --------------------------------------------------------------------

@Composable
@Suppress("LongMethod")
private fun ChannelScreen(
    service: ChatService,
    channel: ChatChannel,
    onBack: () -> Unit,
) {
    val scope = rememberCoroutineScope()
    var draft by remember { mutableStateOf("") }
    val listState = rememberLazyListState()
    // Live updates over SSE — reload messages on each node event for this context (no polling).
    LaunchedEffect(channel.contextId) {
        service.loadMessages(channel)
        runCatching { service.eventStream(channel).collect { service.loadMessages(channel) } }
    }
    LaunchedEffect(service.messages.size) {
        if (service.messages.isNotEmpty()) listState.animateScrollToItem(service.messages.size - 1)
    }
    val canSend = draft.isNotBlank()

    Column(Modifier.fillMaxSize().background(Cal.bg).imePadding()) {
        TopBar("#${channel.name}", onBack = onBack)
        LazyColumn(
            state = listState,
            modifier = Modifier.weight(1f).fillMaxWidth().padding(horizontal = 12.dp),
        ) {
            item { Spacer(Modifier.size(12.dp)) }
            if (service.messages.isEmpty()) {
                item {
                    Column(
                        Modifier.fillMaxWidth().padding(top = 60.dp),
                        horizontalAlignment = Alignment.CenterHorizontally,
                        verticalArrangement = Arrangement.spacedBy(10.dp),
                    ) {
                        IconTile(Icons.Filled.Tag, accent = true, size = 52.dp)
                        Text("This is the start of #${channel.name}", color = Cal.text, fontSize = 17.sp, fontWeight = FontWeight.SemiBold)
                        Text("Say hello. Everyone in the space can read it.", color = Cal.textDim, fontSize = 13.sp)
                    }
                }
            }
            itemsIndexed(service.messages, key = { _, m -> m.id }) { i, message ->
                val previous = service.messages.getOrNull(i - 1)
                val showsHeader =
                    previous == null ||
                        previous.sender != message.sender ||
                        millis(message.timestamp) - millis(previous.timestamp) > RUN_GAP_MS
                MessageBubble(message, mine = isMine(service, message), showsHeader = showsHeader)
            }
            item { Spacer(Modifier.size(12.dp)) }
        }
        service.sendError?.let { error ->
            Callout(
                error,
                kind = CalloutKind.DANGER,
                modifier = Modifier.padding(horizontal = 12.dp, vertical = 6.dp).testTag("sendError"),
            )
        }
        Row(
            Modifier
                .fillMaxWidth()
                .background(Cal.surface)
                .navigationBarsPadding()
                .padding(horizontal = 12.dp, vertical = 8.dp),
            verticalAlignment = Alignment.Bottom,
        ) {
            OutlinedTextField(
                value = draft,
                onValueChange = { draft = it },
                placeholder = { Text("Message #${channel.name}", color = Cal.textFaint) },
                maxLines = COMPOSER_MAX_LINES,
                shape = RoundedCornerShape(24.dp),
                modifier = Modifier.weight(1f).testTag("messageField"),
                colors =
                    OutlinedTextFieldDefaults.colors(
                        focusedContainerColor = Cal.surface,
                        unfocusedContainerColor = Cal.surface,
                        focusedBorderColor = Cal.accentInk.copy(alpha = 0.4f),
                        unfocusedBorderColor = Cal.border,
                        cursorColor = Cal.text,
                    ),
            )
            Spacer(Modifier.width(10.dp))
            Box(
                Modifier
                    .padding(bottom = 6.dp)
                    .size(44.dp)
                    .clip(CircleShape)
                    .background(if (canSend) Cal.lime else Cal.bgSubtle)
                    .clickable(enabled = canSend) {
                        val text = draft.trim()
                        draft = ""
                        scope.launch { service.sendMessage(channel, text) }
                    }.testTag("sendMessage"),
                contentAlignment = Alignment.Center,
            ) {
                Icon(Icons.Filled.ArrowUpward, "Send", tint = if (canSend) Cal.onLime else Cal.textFaint, modifier = Modifier.size(20.dp))
            }
        }
    }
}

/** Mine when the sender's registered name is ours (senders are base58 identities, executors hex). */
private fun isMine(
    service: ChatService,
    message: ChatMessage,
): Boolean = message.senderUsername == service.username

/**
 * A chat bubble: mine on the right in lime, others on the left in white with their avatar and
 * name on the first message of a run.
 */
@Composable
private fun MessageBubble(
    message: ChatMessage,
    mine: Boolean,
    showsHeader: Boolean,
) {
    val name = message.senderUsername.ifEmpty { message.sender.take(SENDER_PREFIX) }
    val shape = RoundedCornerShape(18.dp)
    Row(
        Modifier.fillMaxWidth().padding(top = if (showsHeader) 10.dp else 2.dp),
        horizontalArrangement = if (mine) Arrangement.End else Arrangement.Start,
        verticalAlignment = Alignment.Bottom,
    ) {
        if (mine) Spacer(Modifier.width(56.dp))
        if (!mine) {
            if (showsHeader) ChatAvatar(name, size = 30.dp) else Spacer(Modifier.width(30.dp))
            Spacer(Modifier.width(8.dp))
        }
        Column(Modifier.weight(1f, fill = false), horizontalAlignment = if (mine) Alignment.End else Alignment.Start) {
            if (showsHeader) {
                Row(Modifier.padding(horizontal = 4.dp, vertical = 3.dp), verticalAlignment = Alignment.CenterVertically) {
                    if (!mine) {
                        Text(name, color = Cal.textDim, fontSize = 12.sp, fontWeight = FontWeight.SemiBold)
                        Spacer(Modifier.width(6.dp))
                    }
                    if (message.timestamp > 0) Text(shortTime(message.timestamp), color = Cal.textFaint, fontSize = 11.sp)
                }
            }
            Box(
                Modifier
                    .clip(shape)
                    .background(if (mine) Cal.lime else Cal.surface)
                    .border(1.dp, if (mine) Cal.limeEdge else Cal.border, shape)
                    .padding(horizontal = 14.dp, vertical = 9.dp),
            ) {
                Text(message.text, color = Cal.text, fontSize = 16.sp, lineHeight = 21.sp)
            }
        }
        if (!mine) Spacer(Modifier.width(56.dp))
    }
}

// ---- Shared pieces ---------------------------------------------------------------------------

/** A large-title header: optional back button and trailing actions on top, the title below. */
@Composable
private fun LargeTitleBar(
    title: String,
    onBack: (() -> Unit)? = null,
    actions: @Composable RowScope.() -> Unit = {},
) {
    Column(
        Modifier
            .fillMaxWidth()
            .background(Cal.bg)
            .statusBarsPadding()
            .padding(horizontal = 8.dp),
    ) {
        Row(Modifier.fillMaxWidth().heightIn(min = 48.dp), verticalAlignment = Alignment.CenterVertically) {
            if (onBack != null) {
                IconButton(onClick = onBack) {
                    Icon(Icons.AutoMirrored.Outlined.ArrowBack, contentDescription = "Back", tint = Cal.accentInk)
                }
            }
            Spacer(Modifier.weight(1f))
            actions()
        }
        Text(
            title,
            color = Cal.text,
            fontSize = 32.sp,
            fontWeight = FontWeight.Bold,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.padding(start = 12.dp, end = 12.dp, bottom = 8.dp),
        )
    }
}

/** The "+" menu: a lime circle that opens [items]; each item gets a `close` to dismiss the menu. */
@Composable
private fun AddMenu(
    tag: String,
    items: @Composable (close: () -> Unit) -> Unit,
) {
    var open by remember { mutableStateOf(false) }
    Box {
        IconButton(onClick = { open = true }, modifier = Modifier.testTag(tag)) {
            Box(Modifier.size(30.dp).clip(CircleShape).background(Cal.lime), contentAlignment = Alignment.Center) {
                Icon(Icons.Filled.Add, contentDescription = "Add", tint = Cal.onLime, modifier = Modifier.size(20.dp))
            }
        }
        DropdownMenu(expanded = open, onDismissRequest = { open = false }, containerColor = Cal.surface) {
            items { open = false }
        }
    }
}

@Composable
private fun MenuItem(
    text: String,
    icon: ImageVector,
    onClick: () -> Unit,
) {
    DropdownMenuItem(
        text = { Text(text, color = Cal.text, fontSize = 15.sp) },
        leadingIcon = { Icon(icon, null, tint = Cal.textDim) },
        onClick = onClick,
    )
}

@Composable
private fun StatusLine(service: ChatService) {
    if (service.status.isEmpty()) return
    Row(Modifier.fillMaxWidth().testTag("chatStatus"), verticalAlignment = Alignment.CenterVertically) {
        val color = if (service.statusIsError) Cal.error else Cal.textFaint
        Icon(if (service.statusIsError) Icons.Outlined.ErrorOutline else Icons.Outlined.CheckCircle, null, tint = color, modifier = Modifier.size(15.dp))
        Spacer(Modifier.width(8.dp))
        Text(service.status, color = color, fontSize = 13.sp)
    }
}

@Composable
private fun BusyOverlay(status: String) {
    Box(Modifier.fillMaxSize().background(Color.Black.copy(alpha = 0.18f)).clickable(enabled = false) {}, contentAlignment = Alignment.Center) {
        Column(
            Modifier
                .widthIn(max = 260.dp)
                .clip(RoundedCornerShape(Cal.radiusCard))
                .background(Cal.surface)
                .padding(24.dp),
            horizontalAlignment = Alignment.CenterHorizontally,
            verticalArrangement = Arrangement.spacedBy(14.dp),
        ) {
            CircularProgressIndicator(color = Cal.accentInk, strokeWidth = 3.dp)
            Text(status.ifEmpty { "Working…" }, color = Cal.text, fontSize = 13.sp, fontWeight = FontWeight.Medium, textAlign = TextAlign.Center)
        }
    }
}

@Composable
private fun NameDialog(
    title: String,
    message: String?,
    placeholder: String,
    onDismiss: () -> Unit,
    onCreate: (String) -> Unit,
) {
    var name by remember { mutableStateOf("") }
    AlertDialog(
        onDismissRequest = onDismiss,
        containerColor = Cal.surface,
        title = { Text(title, color = Cal.text, fontWeight = FontWeight.SemiBold) },
        text = {
            Column(verticalArrangement = Arrangement.spacedBy(12.dp)) {
                if (message != null) Text(message, color = Cal.textDim, fontSize = 14.sp)
                OutlinedTextField(
                    value = name,
                    onValueChange = { name = it },
                    placeholder = { Text(placeholder, color = Cal.textFaint) },
                    singleLine = true,
                    shape = RoundedCornerShape(Cal.radiusControl),
                    modifier = Modifier.fillMaxWidth().testTag("createField"),
                    colors = fieldColors(),
                )
            }
        },
        confirmButton = {
            TextButton(onClick = { onCreate(name.trim()) }, enabled = name.isNotBlank()) {
                Text("Create", color = if (name.isNotBlank()) Cal.accentInk else Cal.textFaint, fontWeight = FontWeight.SemiBold)
            }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel", color = Cal.textDim) } },
    )
}

@Composable
private fun JoinDialog(
    service: ChatService,
    onDismiss: () -> Unit,
) {
    val scope = rememberCoroutineScope()
    val clipboard = LocalClipboardManager.current
    var code by remember { mutableStateOf("") }
    Dialog(onDismissRequest = { if (!service.busy) onDismiss() }) {
        Column(
            Modifier.clip(RoundedCornerShape(Cal.radiusCard)).background(Cal.bg).padding(18.dp),
            verticalArrangement = Arrangement.spacedBy(12.dp),
        ) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text("Join a space", color = Cal.text, fontSize = 18.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f))
                TextButton(onClick = onDismiss, enabled = !service.busy) { Text("Close", color = Cal.accentInk) }
            }
            Text("Paste an invite code", color = Cal.text, fontSize = 15.sp, fontWeight = FontWeight.Medium)
            Row(verticalAlignment = Alignment.CenterVertically) {
                OutlinedTextField(
                    value = code,
                    onValueChange = { code = it },
                    placeholder = { Text("invite code", color = Cal.textFaint, fontFamily = FontFamily.Monospace) },
                    singleLine = true,
                    enabled = !service.busy,
                    shape = RoundedCornerShape(Cal.radiusControl),
                    modifier = Modifier.weight(1f).testTag("joinField"),
                    colors = fieldColors(),
                )
                IconButton(onClick = { clipboard.getText()?.let { code = it.text } }, enabled = !service.busy) {
                    Icon(Icons.Outlined.ContentPaste, "Paste", tint = Cal.accentInk)
                }
            }
            Box(contentAlignment = Alignment.Center) {
                CalPrimaryButton(
                    if (service.busy) "Joining…" else "Join space",
                    onClick = {
                        // Strip any whitespace/newlines a paste may have introduced.
                        scope.launch { if (service.joinSpace(code.trim())) onDismiss() }
                    },
                    enabled = !service.busy && code.isNotBlank(),
                    modifier = Modifier.fillMaxWidth(),
                )
            }
            if (service.status.isNotEmpty()) {
                Text(service.status, color = if (service.statusIsError) Cal.error else Cal.textDim, fontSize = 12.sp)
            }
        }
    }
}

@Composable
private fun InviteDialog(
    code: String,
    spaceName: String,
    onDismiss: () -> Unit,
) {
    val context = LocalContext.current
    val clipboard = LocalClipboardManager.current
    var copied by remember { mutableStateOf(false) }
    Dialog(onDismissRequest = onDismiss) {
        Column(
            Modifier.clip(RoundedCornerShape(Cal.radiusCard)).background(Cal.bg).padding(18.dp),
            verticalArrangement = Arrangement.spacedBy(14.dp),
        ) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text("Invite", color = Cal.text, fontSize = 18.sp, fontWeight = FontWeight.SemiBold, modifier = Modifier.weight(1f))
                TextButton(onClick = onDismiss) { Text("Done", color = Cal.accentInk, fontWeight = FontWeight.SemiBold) }
            }
            Text("Share this invite code so someone can join “$spaceName”.", color = Cal.textDim, fontSize = 13.sp)
            Text(
                code,
                color = Cal.text,
                fontFamily = FontFamily.Monospace,
                fontSize = 12.sp,
                maxLines = INVITE_MAX_LINES,
                overflow = TextOverflow.Ellipsis,
                modifier =
                    Modifier
                        .fillMaxWidth()
                        .clip(RoundedCornerShape(10.dp))
                        .background(Cal.surfaceSunken)
                        .border(1.dp, Cal.border, RoundedCornerShape(10.dp))
                        .padding(12.dp)
                        .testTag("inviteCode"),
            )
            Row(horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                CalSecondaryButton(
                    if (copied) "Copied" else "Copy",
                    onClick = {
                        clipboard.setText(AnnotatedString(code))
                        copied = true
                    },
                    icon = if (copied) Icons.Outlined.CheckCircle else Icons.Outlined.ContentCopy,
                    modifier = Modifier.weight(1f),
                )
                CalSecondaryButton(
                    "Share",
                    onClick = {
                        val send = Intent(Intent.ACTION_SEND).setType("text/plain").putExtra(Intent.EXTRA_TEXT, code)
                        context.startActivity(Intent.createChooser(send, "Share invite"))
                    },
                    icon = Icons.Outlined.Share,
                    modifier = Modifier.weight(1f),
                )
            }
        }
    }
}

@Composable
private fun fieldColors() =
    OutlinedTextFieldDefaults.colors(
        focusedContainerColor = Cal.surface,
        unfocusedContainerColor = Cal.surface,
        disabledContainerColor = Cal.surfaceSunken,
        focusedBorderColor = Cal.text,
        unfocusedBorderColor = Cal.borderStrong,
        cursorColor = Cal.text,
    )

private val timeFormat = SimpleDateFormat("HH:mm", Locale.getDefault())

/**
 * mero-chat (3.1.29+) stores message timestamps in seconds. Older messages may still carry
 * milliseconds, so anything already past the seconds range is taken as-is.
 */
private fun millis(timestamp: Long): Long = if (timestamp < SECONDS_RANGE_LIMIT) timestamp * MILLIS_PER_SECOND else timestamp

private fun shortTime(timestamp: Long): String = timeFormat.format(Date(millis(timestamp)))

private const val SECONDS_RANGE_LIMIT = 100_000_000_000L
private const val MILLIS_PER_SECOND = 1000L

/** A new header (avatar, name, time) after five quiet minutes, as in the Swift sample. */
private const val RUN_GAP_MS = 300_000L
private const val SENDER_PREFIX = 6
private const val COMPOSER_MAX_LINES = 5
private const val INVITE_MAX_LINES = 8
