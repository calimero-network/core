import MeroKit
import SwiftUI
import UIKit

// MARK: - Chat home (spaces)

struct ChatHomeView: View {
    @ObservedObject var service: ChatService
    @State private var newSpace = ""
    @State private var showNewSpace = false
    @State private var showJoin = false

    var body: some View {
        NavigationStack {
            ZStack {
                Cal.bg.ignoresSafeArea()
                if service.appId == nil {
                    installGate
                } else {
                    spacesList
                }
                if service.busy { busyOverlay }
            }
            .navigationTitle("Chat")
            .navigationBarTitleDisplayMode(.large)
            .toolbar {
                if service.appId != nil {
                    ToolbarItem(placement: .topBarTrailing) {
                        Menu {
                            if service.offerNewSpace {
                                Button {
                                    showNewSpace = true
                                } label: {
                                    Label("New space", systemImage: "plus.bubble")
                                }
                            }
                            Button {
                                showJoin = true
                            } label: {
                                Label("Join with an invite", systemImage: "ticket")
                            }
                            Divider()
                            Button {
                                Task { await service.loadSpaces() }
                            } label: {
                                Label("Refresh", systemImage: "arrow.clockwise")
                            }
                        } label: {
                            Image(systemName: "plus.circle.fill")
                                .font(.title3)
                                .symbolRenderingMode(.palette)
                                .foregroundStyle(Cal.text, Cal.lime)
                        }
                        .accessibilityIdentifier("chatAdd")
                    }
                }
            }
            .tint(Cal.accentInk)
        }
        .task {
            let env = ProcessInfo.processInfo.environment
            // e2e hook: with E2E_JOIN=<invite json> set, auto-install then join —
            // lets the multi-user harness hand a guest an invite without typing it.
            if let invite = env["E2E_JOIN"], !invite.isEmpty, service.appId == nil {
                await service.setup()
                await service.joinSpace(invite)
            } else {
                // Skip the install gate if mero-chat is already installed on this node.
                await service.detectInstalled()
            }
        }
        .alert("New space", isPresented: $showNewSpace) {
            TextField("Space name", text: $newSpace)
            Button("Create") {
                let n = newSpace; newSpace = ""; Task { await service.createSpace(n) }
            }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("A space holds channels and the people you invite.")
        }
        .sheet(isPresented: $showJoin) {
            JoinSheet(service: service)
        }
    }

    private var busyOverlay: some View {
        ZStack {
            Color.black.opacity(0.18).ignoresSafeArea()
            VStack(spacing: 14) {
                ProgressView().tint(Cal.accentInk).scaleEffect(1.2)
                Text(service.status.isEmpty ? "Working…" : service.status)
                    .font(.footnote.weight(.medium)).foregroundColor(Cal.text)
                    .multilineTextAlignment(.center)
            }
            .padding(24)
            .frame(maxWidth: 260)
            .background(.regularMaterial)
            .clipShape(RoundedRectangle(cornerRadius: Cal.cardRadius))
            .shadow(color: .black.opacity(0.12), radius: 24, y: 10)
        }
        .transition(.opacity)
    }

    private var installGate: some View {
        ScrollView {
            VStack(spacing: 22) {
                ZStack {
                    Circle().fill(Cal.accentSoft).frame(width: 112, height: 112)
                    Image(systemName: "bubble.left.and.bubble.right.fill")
                        .font(.system(size: 44))
                        .foregroundColor(Cal.accentInk)
                }
                .padding(.top, 48)
                VStack(spacing: 8) {
                    Text("Set up Mero Chat")
                        .font(.system(.title2, design: .rounded).weight(.bold))
                        .foregroundColor(Cal.text)
                    Text(
                        "Installs Mero Chat (com.calimero.chat) from the Calimero app registry onto your node. "
                            + "Messages live on your node, not on a server."
                    )
                    .font(.callout)
                    .foregroundColor(Cal.textDim)
                    .multilineTextAlignment(.center)
                    .fixedSize(horizontal: false, vertical: true)
                }
                Button {
                    Task { await service.setup() }
                } label: {
                    HStack(spacing: 10) {
                        if service.busy {
                            ProgressView().tint(Cal.text)
                        } else {
                            Image(systemName: "arrow.down.circle.fill")
                        }
                        Text("Install Mero Chat")
                    }
                }
                .buttonStyle(CalPrimaryButtonStyle(enabled: !service.busy))
                .disabled(service.busy)
                .accessibilityIdentifier("installChat")
                statusLine
            }
            .frame(maxWidth: 420)
            .frame(maxWidth: .infinity)
            .padding(.horizontal, 28)
        }
    }

    private var spacesList: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 14) {
                statusLine
                if service.spaces.isEmpty, service.spacesLoad == .pending {
                    EmptyState(
                        icon: "bubble.left.and.bubble.right", title: "Loading spaces…",
                        message: "Waiting for your session to connect.")
                } else if service.spaces.isEmpty, service.spacesLoad == .failed {
                    // A failed listing is not an empty one: no "create" here, only a retry.
                    EmptyState(
                        icon: "bubble.left.and.bubble.right", title: "Couldn't load your spaces",
                        message: "Your spaces may still be there. Try again once the connection is back.")
                    Button {
                        Task { await service.loadSpaces() }
                    } label: {
                        Label("Try again", systemImage: "arrow.clockwise")
                    }
                    .buttonStyle(CalSecondaryButtonStyle())
                    .frame(maxWidth: .infinity)
                    .accessibilityIdentifier("chatRetrySpaces")
                } else if service.spaces.isEmpty {
                    EmptyState(
                        icon: "bubble.left.and.bubble.right", title: "No spaces yet",
                        message: service.canCreateSpaces
                            ? "Create a space with +, or join one with an invite."
                            : "Join a space with an invite from its admin. Tap + to paste one.")
                } else {
                    CalSection(title: "Spaces") {
                        ForEach(Array(service.spaces.enumerated()), id: \.element.id) { idx, space in
                            NavigationLink {
                                ChannelsView(service: service, space: space)
                            } label: {
                                HStack(spacing: 12) {
                                    SpaceAvatar(name: space.name)
                                    VStack(alignment: .leading, spacing: 2) {
                                        Text(space.name).font(.body.weight(.semibold)).foregroundColor(Cal.text)
                                        Text("Space").font(.footnote).foregroundColor(Cal.textFaint)
                                    }
                                    Spacer()
                                    Image(systemName: "chevron.right")
                                        .font(.footnote.weight(.semibold)).foregroundColor(Cal.textFaint)
                                }
                                .padding(.horizontal, 14)
                                .frame(minHeight: 66)
                                .contentShape(Rectangle())
                            }
                            .buttonStyle(.plain)
                            if idx < service.spaces.count - 1 { RowDivider() }
                        }
                    }
                }
            }
            .padding(.horizontal, Cal.screenPad + 4)
            .padding(.vertical, 8)
        }
        .refreshable { await service.loadSpaces() }
        .task { await service.loadSpaces() }
    }

    @ViewBuilder private var statusLine: some View {
        if !service.status.isEmpty {
            HStack(spacing: 8) {
                Image(systemName: service.statusIsError ? "exclamationmark.circle" : "checkmark.circle")
                Text(service.status)
            }
            .font(.footnote)
            .foregroundColor(service.statusIsError ? Cal.error : Cal.textFaint)
            .frame(maxWidth: .infinity, alignment: .leading)
            .accessibilityElement(children: .combine)
            .accessibilityIdentifier("chatStatus")
        }
    }
}

/// A rounded initials tile for a space (deterministic colour per name).
struct SpaceAvatar: View {
    let name: String
    var size: CGFloat = 42
    var body: some View {
        let tone = AvatarTone.pick(name)
        Text(String(name.prefix(1)).uppercased())
            .font(.system(size: size * 0.42, weight: .bold, design: .rounded))
            .foregroundColor(tone.fg)
            .frame(width: size, height: size)
            .background(RoundedRectangle(cornerRadius: size * 0.3).fill(tone.bg))
    }
}

// MARK: - Channels in a space

struct ChannelsView: View {
    @ObservedObject var service: ChatService
    let space: ChatSpace
    @State private var newChannel = ""
    @State private var showNew = false
    @State private var openChannel = true
    @State private var invite: String?
    @State private var inviteError = false

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 14) {
                if !service.status.isEmpty {
                    HStack(spacing: 8) {
                        if service.busy { ProgressView().tint(Cal.accentInk) }
                        Text(service.status).font(.footnote)
                            .foregroundColor(service.statusIsError ? Cal.error : Cal.textFaint)
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
                if service.channels.isEmpty && !service.busy {
                    VStack(spacing: 14) {
                        EmptyState(
                            icon: "number", title: "No channels yet",
                            message: "If you just joined, channels sync from the inviter. Or create one with +.")
                        Button {
                            Task { await service.resync(space) }
                        } label: {
                            Label("Sync now", systemImage: "arrow.triangle.2.circlepath")
                        }
                        .buttonStyle(CalSecondaryButtonStyle())
                    }
                } else if !service.channels.isEmpty {
                    CalSection(title: "Channels") {
                        ForEach(Array(service.channels.enumerated()), id: \.element.id) { idx, ch in
                            NavigationLink {
                                ChannelView(service: service, channel: ch)
                            } label: {
                                HStack(spacing: 12) {
                                    IconTile(
                                        systemName: ch.kind == "Dm" ? "person.fill" : "number", accent: true, size: 38)
                                    Text(ch.name).font(.body.weight(.semibold)).foregroundColor(Cal.text)
                                    Spacer()
                                    Image(systemName: "chevron.right")
                                        .font(.footnote.weight(.semibold)).foregroundColor(Cal.textFaint)
                                }
                                .padding(.horizontal, 14)
                                .frame(minHeight: 60)
                                .contentShape(Rectangle())
                            }
                            .buttonStyle(.plain)
                            if idx < service.channels.count - 1 { RowDivider() }
                        }
                    }
                }
            }
            .padding(.horizontal, Cal.screenPad + 4)
            .padding(.vertical, 12)
        }
        .background(Cal.bg)
        .navigationTitle(space.name)
        .navigationBarTitleDisplayMode(.large)
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Menu {
                    Button {
                        showNew = true
                    } label: {
                        Label("New channel", systemImage: "number")
                    }
                    if service.canInvite {
                        Button {
                            Task {
                                let code = await service.makeInvite(space)
                                if let code { invite = code } else { inviteError = true }
                            }
                        } label: {
                            Label("Invite people", systemImage: "person.badge.plus")
                        }
                    }
                    Button {
                        Task { await service.resync(space) }
                    } label: {
                        Label("Sync now", systemImage: "arrow.triangle.2.circlepath")
                    }
                } label: {
                    Image(systemName: "plus.circle.fill")
                        .font(.title3)
                        .symbolRenderingMode(.palette)
                        .foregroundStyle(Cal.text, Cal.lime)
                }
                .accessibilityIdentifier("channelAdd")
            }
        }
        .task { await service.loadChannels(space) }
        .refreshable { await service.loadChannels(space) }
        .alert("New channel", isPresented: $showNew) {
            TextField("channel-name", text: $newChannel)
            Button("Create") {
                let n = newChannel; newChannel = ""
                Task { await service.createChannel(in: space, name: n, open: openChannel) }
            }
            Button("Cancel", role: .cancel) {}
        }
        .sheet(item: Binding(get: { invite.map { InviteBox(text: $0) } }, set: { _ in invite = nil })) { box in
            InviteSheet(text: box.text, spaceName: space.name)
        }
        .alert("Couldn't create invite", isPresented: $inviteError) {
            Button("OK", role: .cancel) {}
        } message: {
            Text(service.status.isEmpty ? "The node did not return an invitation." : service.status)
        }
    }
}

private struct InviteBox: Identifiable { let id = UUID(); let text: String }

// MARK: - A channel's messages

struct ChannelView: View {
    @ObservedObject var service: ChatService
    let channel: ChatChannel
    @State private var draft = ""
    @FocusState private var composing: Bool

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(spacing: 2) {
                    if service.messages.isEmpty {
                        VStack(spacing: 10) {
                            IconTile(systemName: "number", accent: true, size: 52)
                            Text("This is the start of #\(channel.name)")
                                .font(.headline).foregroundColor(Cal.text)
                            Text("Say hello. Everyone in the space can read it.")
                                .font(.footnote).foregroundColor(Cal.textDim)
                        }
                        .padding(.top, 60)
                    }
                    ForEach(Array(service.messages.enumerated()), id: \.element.id) { idx, message in
                        let previous = idx > 0 ? service.messages[idx - 1] : nil
                        MessageBubble(
                            message: message,
                            isMine: isMine(message),
                            showsHeader: previous.map {
                                $0.sender != message.sender || message.timestamp - $0.timestamp > 300
                            }
                                ?? true
                        )
                        .id(message.id)
                    }
                }
                .padding(.horizontal, 12)
                .padding(.vertical, 12)
            }
            .scrollDismissesKeyboard(.interactively)
            .onChange(of: service.messages.count) { _ in
                if let last = service.messages.last { withAnimation { proxy.scrollTo(last.id, anchor: .bottom) } }
            }
        }
        .background(Cal.bg.ignoresSafeArea())
        .safeAreaInset(edge: .bottom) {
            VStack(spacing: 0) {
                if service.statusIsError && !service.status.isEmpty {
                    Callout(tone: .danger, text: service.status)
                        .padding(.horizontal, 12)
                        .padding(.bottom, 6)
                        .accessibilityIdentifier("channelError")
                }
                composer
            }
        }
        .navigationTitle("#\(channel.name)")
        .navigationBarTitleDisplayMode(.inline)
        .toolbar(.hidden, for: .tabBar)
        .task(id: channel.id) {
            // Live updates over SSE — reload messages on each node event for this
            // context (no polling). Cancelling the task closes the SSE stream.
            await service.loadMessages(channel)
            do {
                for try await _ in service.eventStream(channel) {
                    await service.loadMessages(channel)
                }
            } catch {}
        }
    }

    private func isMine(_ message: ChatMessage) -> Bool {
        (!channel.executorId.isEmpty && message.sender == channel.executorId)
            || message.senderUsername == service.username
    }

    private var canSend: Bool { !draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }

    private var composer: some View {
        HStack(alignment: .bottom, spacing: 10) {
            TextField("Message #\(channel.name)", text: $draft, axis: .vertical)
                .lineLimit(1...5)
                .font(.body).foregroundColor(Cal.text)
                .focused($composing)
                .padding(.horizontal, 16).padding(.vertical, 11)
                .background(Capsule().fill(Cal.surface))
                .overlay(Capsule().stroke(composing ? Cal.accentInk.opacity(0.4) : Cal.border, lineWidth: 1))
                .accessibilityIdentifier("messageField")
            Button {
                let t = draft.trimmingCharacters(in: .whitespacesAndNewlines); draft = ""
                Task { await service.sendMessage(channel, t) }
            } label: {
                Image(systemName: "arrow.up")
                    .font(.system(size: 17, weight: .bold))
                    .foregroundColor(canSend ? Cal.text : Cal.textFaint)
                    .frame(width: 44, height: 44)
                    .background(Circle().fill(canSend ? Cal.lime : Cal.bgSubtle))
            }
            .disabled(!canSend)
            .animation(.easeInOut(duration: 0.15), value: canSend)
            .accessibilityIdentifier("sendMessage")
        }
        .padding(.horizontal, 12).padding(.top, 8).padding(.bottom, 8)
        .background(.bar)
    }
}

/// A chat bubble: mine on the right in lime, others on the left in white with
/// their avatar and name on the first message of a run.
struct MessageBubble: View {
    let message: ChatMessage
    let isMine: Bool
    var showsHeader = true

    private var name: String {
        message.senderUsername.isEmpty ? String(message.sender.prefix(6)) : message.senderUsername
    }

    var body: some View {
        HStack(alignment: .bottom, spacing: 8) {
            if isMine { Spacer(minLength: 56) }
            if !isMine {
                if showsHeader { ChatAvatar(name: name, size: 30) } else { Color.clear.frame(width: 30, height: 1) }
            }
            VStack(alignment: isMine ? .trailing : .leading, spacing: 3) {
                if showsHeader {
                    HStack(spacing: 6) {
                        if !isMine { Text(name).font(.caption.weight(.semibold)).foregroundColor(Cal.textDim) }
                        Text(ChatTime.short(message.timestamp)).font(.caption2).foregroundColor(Cal.textFaint)
                    }
                    .padding(.horizontal, 4)
                }
                Text(message.text)
                    .font(.body)
                    .foregroundColor(Cal.text)
                    .padding(.horizontal, 14).padding(.vertical, 9)
                    .background(
                        RoundedRectangle(cornerRadius: 18, style: .continuous)
                            .fill(isMine ? Cal.lime : Cal.surface)
                    )
                    .overlay(
                        RoundedRectangle(cornerRadius: 18, style: .continuous)
                            .stroke(isMine ? Color.black.opacity(0.05) : Cal.border, lineWidth: 1)
                    )
                    .fixedSize(horizontal: false, vertical: true)
                    .textSelection(.enabled)
            }
            if !isMine { Spacer(minLength: 56) }
        }
        .padding(.top, showsHeader ? 10 : 0)
        .frame(maxWidth: .infinity, alignment: isMine ? .trailing : .leading)
    }
}

// MARK: - Invite / Join sheets

struct InviteSheet: View {
    let text: String
    var spaceName: String = "space"
    @Environment(\.dismiss) private var dismiss
    @State private var copied = false

    var body: some View {
        NavigationStack {
            VStack(spacing: 14) {
                Text("Share this invite code so someone can join “\(spaceName)”.")
                    .font(.footnote).foregroundColor(Cal.textDim)
                    .frame(maxWidth: .infinity, alignment: .leading)
                ScrollView {
                    Text(text).font(Cal.mono).foregroundColor(Cal.text).textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading).padding(12)
                }
                .frame(maxHeight: 220)
                .background(Cal.surface2)
                .overlay(RoundedRectangle(cornerRadius: 10).stroke(Cal.border, lineWidth: 1)).cornerRadius(10)
                HStack(spacing: 10) {
                    Button {
                        UIPasteboard.general.string = text
                        copied = true
                    } label: {
                        Label(copied ? "Copied" : "Copy", systemImage: copied ? "checkmark" : "doc.on.doc")
                    }
                    .buttonStyle(CalSecondaryButtonStyle())
                    ShareLink(item: text) { Label("Share", systemImage: "square.and.arrow.up") }
                        .buttonStyle(CalSecondaryButtonStyle())
                }
                Spacer()
            }
            .padding(16)
            .background(Cal.bg)
            .navigationTitle("Invite")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) { Button("Done") { dismiss() }.foregroundColor(Cal.accentInk) }
            }
        }
        .tint(Cal.accentInk)
    }
}

struct JoinSheet: View {
    @ObservedObject var service: ChatService
    @Environment(\.dismiss) private var dismiss
    @State private var text = ""

    var body: some View {
        NavigationStack {
            VStack(alignment: .leading, spacing: 12) {
                Text("Paste an invite code").font(.headline).foregroundColor(Cal.text)
                // Single-line: an invite code is one line, so a one-liner avoids
                // stray line breaks that could mangle the code, and a Paste button
                // makes the (long, unreadable) code easy to drop in.
                HStack(spacing: 8) {
                    TextField("invite code", text: $text)
                        .font(Cal.mono).foregroundColor(Cal.text)
                        .textInputAutocapitalization(.never).autocorrectionDisabled()
                        .lineLimit(1).truncationMode(.middle)
                        .padding(.horizontal, 12).padding(.vertical, 11)
                        .background(Cal.surface2)
                        .overlay(RoundedRectangle(cornerRadius: 10).stroke(Cal.border, lineWidth: 1))
                        .cornerRadius(10)
                        .disabled(service.busy)
                        .accessibilityIdentifier("joinField")
                    Button {
                        if let clip = UIPasteboard.general.string { text = clip }
                    } label: {
                        Image(systemName: "doc.on.clipboard").font(.body)
                    }
                    .foregroundColor(Cal.accentInk)
                    .disabled(service.busy)
                }
                Button {
                    Task {
                        // Strip any whitespace/newlines a paste may have introduced.
                        if await service.joinSpace(text.trimmingCharacters(in: .whitespacesAndNewlines)) {
                            dismiss()
                        }
                    }
                } label: {
                    if service.busy {
                        HStack(spacing: 8) {
                            ProgressView().tint(Cal.text); Text("Joining…")
                        }
                    } else {
                        Text("Join space")
                    }
                }
                .buttonStyle(CalPrimaryButtonStyle())
                .disabled(service.busy || text.trimmingCharacters(in: .whitespaces).isEmpty)

                if !service.status.isEmpty {
                    HStack(spacing: 8) {
                        if service.busy { ProgressView().tint(Cal.accentInk) }
                        Text(service.status)
                            .font(.caption)
                            .foregroundColor(service.statusIsError ? Cal.error : Cal.textDim)
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
                Spacer()
            }
            .padding(16).background(Cal.bg)
            .navigationTitle("Join a space").navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Close") { dismiss() }.foregroundColor(Cal.accentInk).disabled(service.busy)
                }
            }
        }
        .tint(Cal.accentInk)
    }
}

// MARK: - Avatars

/// Calimero avatar tones, chosen by a stable hash of the name.
enum AvatarTone {
    static func pick(_ name: String) -> (bg: Color, fg: Color) {
        let tones: [(UInt, UInt)] = [
            (0xF0FFD6, 0x4A7300), (0xE6EEFB, 0x1D4F9F), (0xFBE9E4, 0x9A3412),
            (0xEFE8FB, 0x5B3AA8), (0xFDF3DC, 0x8A5300), (0xE2F4F1, 0x116A5C),
        ]
        var hash = 5381
        for byte in name.utf8 { hash = ((hash << 5) &+ hash) &+ Int(byte) }
        let pick = tones[((hash % tones.count) + tones.count) % tones.count]
        return (Color(hex: pick.0), Color(hex: pick.1))
    }
}

/// A coloured initials avatar.
struct ChatAvatar: View {
    let name: String
    var size: CGFloat = 32

    private var initials: String {
        let parts = name.split(separator: " ")
        let joined = parts.prefix(2).map { String($0.prefix(1)) }.joined()
        return (joined.isEmpty ? String(name.prefix(1)) : joined).uppercased()
    }

    var body: some View {
        let tone = AvatarTone.pick(name)
        Text(initials)
            .font(.system(size: size * 0.4, weight: .bold, design: .rounded))
            .foregroundColor(tone.fg)
            .frame(width: size, height: size)
            .background(tone.bg)
            .clipShape(Circle())
    }
}

enum ChatTime {
    private static let formatter: DateFormatter = {
        let f = DateFormatter()
        f.dateFormat = "HH:mm"
        return f
    }()
    /// mero-chat timestamps are seconds since the epoch. Older messages sent
    /// in milliseconds are still shown at the right time.
    static func short(_ timestamp: Int) -> String {
        let seconds = timestamp > 100_000_000_000 ? Double(timestamp) / 1000 : Double(timestamp)
        return formatter.string(from: Date(timeIntervalSince1970: seconds))
    }
}

/// Dashed empty state: icon tile, title, body.
struct EmptyState: View {
    let icon: String
    let title: String
    let message: String
    var body: some View {
        VStack(spacing: 10) {
            IconTile(systemName: icon, size: 40)
            Text(title).font(.subheadline.weight(.semibold)).foregroundColor(Cal.text)
            Text(message).font(.footnote).foregroundColor(Cal.textDim).multilineTextAlignment(.center)
        }
        .frame(maxWidth: .infinity)
        .padding(.vertical, 32).padding(.horizontal, 20)
        .background(RoundedRectangle(cornerRadius: Cal.cardRadius).fill(Cal.surface.opacity(0.6)))
        .overlay(
            RoundedRectangle(cornerRadius: Cal.cardRadius)
                .stroke(Cal.borderStrong, style: StrokeStyle(lineWidth: 1, dash: [5, 4])))
    }
}
