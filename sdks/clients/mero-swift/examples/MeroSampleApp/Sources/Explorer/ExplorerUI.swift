import MeroKit
import MeroKitUI
import SwiftUI
import UIKit

// MARK: - Root: routes Sign-in ⇄ the app on auth state

struct ExplorerRootView: View {
    @EnvironmentObject private var session: MeroSession
    var body: some View {
        ZStack {
            Cal.bg.ignoresSafeArea()
            if session.isAuthenticated {
                MainTabView()
                    .transition(.opacity)
            } else {
                CalimeroLoginView()
                    .transition(.opacity)
            }
        }
        .animation(.easeInOut(duration: 0.25), value: session.isAuthenticated)
        .tint(Cal.accentInk)
        .task { await session.start() }
        .onOpenURL { url in Task { await session.handle(url) } }
    }
}

// MARK: - Sign in

/// Calimero Cloud first: the cluster's URL, then a passkey on this device (the
/// node is attested before anything is sent). Under it, "Manually enter node
/// URL" connects to a development cluster (`merod local`) with mock
/// attestation and a software passkey kept for that node.
struct CalimeroLoginView: View {
    @EnvironmentObject private var session: MeroSession
    @State private var showLogs = false
    @State private var manual = false
    @State private var clusterURL = ""
    @State private var nodeURL = ""
    @State private var name = ""
    @FocusState private var focus: Field?

    private enum Field { case cluster, url, name }

    var body: some View {
        ZStack(alignment: .top) {
            Cal.bg.ignoresSafeArea()
            LimeGlow()
            GeometryReader { geo in
                ScrollView {
                    VStack(spacing: 28) {
                        hero
                            .padding(.top, manual ? 40 : 96)
                        VStack(spacing: 14) {
                            if let error = session.errorMessage {
                                Callout(tone: .danger, text: error)
                                    .accessibilityElement(children: .combine)
                                    .accessibilityIdentifier("loginError")
                                    .transition(.opacity)
                            }
                            if manual {
                                nodeForm.transition(.move(edge: .bottom).combined(with: .opacity))
                            } else {
                                cloudButton.transition(.opacity)
                            }
                            Button {
                                withAnimation(.spring(response: 0.35, dampingFraction: 0.85)) {
                                    manual.toggle()
                                    session.errorMessage = nil
                                }
                                if manual { focus = .url }
                            } label: {
                                Text(manual ? "Use Calimero Cloud instead" : "Manually enter node URL")
                                    .underline(manual == false, color: Cal.textFaint.opacity(0.5))
                            }
                            .buttonStyle(CalTextButtonStyle())
                            .accessibilityIdentifier(manual ? "useCloud" : "manualNode")
                        }
                        Spacer(minLength: 24)
                        footer
                    }
                    .frame(maxWidth: 440, minHeight: geo.size.height - 16)
                    .frame(maxWidth: .infinity)
                    .padding(.horizontal, 24)
                    .padding(.bottom, 16)
                }
                .scrollDismissesKeyboard(.interactively)
            }
        }
        .sheet(isPresented: $showLogs) { LogsView() }
        .onAppear {
            if clusterURL.isEmpty { clusterURL = session.lastClusterURL }
            if nodeURL.isEmpty { nodeURL = session.lastNodeURL }
            if name.isEmpty { name = session.lastName }
        }
    }

    private var hero: some View {
        VStack(spacing: 18) {
            Image("CalimeroIcon")
                .resizable()
                .scaledToFit()
                .padding(16)
                .frame(width: 76, height: 76)
                .background(RoundedRectangle(cornerRadius: 22).fill(Cal.lime))
                .overlay(RoundedRectangle(cornerRadius: 22).stroke(Color.black.opacity(0.06), lineWidth: 1))
                .shadow(color: Cal.accentInk.opacity(0.25), radius: 18, y: 8)
                .accessibilityHidden(true)
            VStack(spacing: 8) {
                Text(manual ? "Connect to a node" : "Welcome to MeroKit")
                    .font(.system(.largeTitle, design: .rounded).weight(.bold))
                    .foregroundColor(Cal.text)
                    .multilineTextAlignment(.center)
                    .accessibilityIdentifier("loginTitle")
                Text(
                    manual
                        ? "Connect to a development cluster, like merod local. Its mock attestation is trusted only here."
                        : "Confidential apps on attested Calimero nodes. Sign in with a passkey on this device."
                )
                .font(.callout)
                .foregroundColor(Cal.textDim)
                .multilineTextAlignment(.center)
                .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    private var cloudButton: some View {
        CalCard(padding: 18) {
            VStack(spacing: 14) {
                CalField(title: "Cluster URL", text: $clusterURL, placeholder: "https://node.your-cluster.example")
                    .keyboardType(.URL)
                    .submitLabel(.go)
                    .focused($focus, equals: .cluster)
                    .onSubmit { signIn(createAccount: false) }
                    .accessibilityIdentifier("clusterURLField")
                Button {
                    signIn(createAccount: false)
                } label: {
                    HStack(spacing: 10) {
                        if session.isLoading {
                            ProgressView().tint(Cal.text)
                        } else {
                            Image(systemName: "person.badge.key.fill").font(.callout)
                        }
                        Text("Sign in with passkey")
                    }
                }
                .buttonStyle(CalPrimaryButtonStyle(enabled: canSignIn))
                .disabled(!canSignIn)
                .accessibilityIdentifier("cloudSignInButton")
                Button("Create an account") { signIn(createAccount: true) }
                    .buttonStyle(CalTextButtonStyle())
                    .disabled(!canSignIn)
                    .accessibilityIdentifier("cloudSignUpButton")
                HStack(spacing: 6) {
                    Image(systemName: "checkmark.shield").font(.caption)
                    Text("The node's TEE attestation is verified first")
                }
                .font(.caption)
                .foregroundColor(Cal.textFaint)
            }
        }
    }

    private var canSignIn: Bool {
        !session.isLoading && !clusterURL.trimmingCharacters(in: .whitespaces).isEmpty
    }

    private func signIn(createAccount: Bool) {
        guard canSignIn else { return }
        focus = nil
        Task { await session.signInWithCloud(clusterURL, createAccount: createAccount) }
    }

    private var nodeForm: some View {
        CalCard(padding: 18) {
            VStack(spacing: 14) {
                CalField(title: "Node URL", text: $nodeURL, placeholder: "http://localhost:2501")
                    .keyboardType(.URL)
                    .submitLabel(.next)
                    .focused($focus, equals: .url)
                    .onSubmit { focus = .name }
                    .accessibilityIdentifier("nodeURLField")
                CalField(title: "Your name", text: $name, placeholder: "dev")
                    .submitLabel(.go)
                    .focused($focus, equals: .name)
                    .onSubmit(connect)
                    .accessibilityIdentifier("nodeUserField")
                Button(action: connect) {
                    HStack(spacing: 10) {
                        if session.isLoading {
                            ProgressView().tint(Cal.text)
                        } else {
                            Image(systemName: "server.rack").font(.callout)
                        }
                        Text("Connect to node")
                    }
                }
                .buttonStyle(CalPrimaryButtonStyle(enabled: canConnect))
                .disabled(!canConnect)
                .accessibilityIdentifier("nodeConnectButton")
                Text("In the Simulator, localhost is your Mac. On a phone, use your Mac's LAN address.")
                    .font(.caption)
                    .foregroundColor(Cal.textFaint)
                    .multilineTextAlignment(.center)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
    }

    private var canConnect: Bool {
        !session.isLoading && !nodeURL.trimmingCharacters(in: .whitespaces).isEmpty
    }

    private func connect() {
        guard canConnect else { return }
        focus = nil
        Task { await session.connectNode(nodeURL, name: name) }
    }

    private var footer: some View {
        Button {
            showLogs = true
        } label: {
            Label("Diagnostics", systemImage: "waveform.path.ecg")
        }
        .buttonStyle(CalTextButtonStyle())
    }
}

// MARK: - The signed-in app: Home, Chat, SDK

struct MainTabView: View {
    enum Tab: Hashable { case home, chat, sdk }
    @EnvironmentObject private var session: MeroSession
    @State private var tab: Tab = .home

    var body: some View {
        TabView(selection: $tab) {
            HomeView(tab: $tab)
                .tabItem { Label("Home", systemImage: "house") }
                .tag(Tab.home)
            Group {
                if let chat = session.chat {
                    ChatHomeView(service: chat)
                } else {
                    Text("Chat is not available for this session.").foregroundColor(Cal.textDim)
                }
            }
            .tabItem { Label("Chat", systemImage: "bubble.left.and.bubble.right") }
            .tag(Tab.chat)
            NavigationStack { SDKListView() }
                .tabItem { Label("SDK", systemImage: "square.grid.2x2") }
                .tag(Tab.sdk)
        }
        .tint(Cal.accentInk)
    }
}

struct HomeView: View {
    @Binding var tab: MainTabView.Tab
    @EnvironmentObject private var session: MeroSession
    @State private var showLogs = false

    var body: some View {
        NavigationStack {
            ZStack(alignment: .top) {
                Cal.bg.ignoresSafeArea()
                LimeGlow().frame(height: 320)
                ScrollView {
                    VStack(alignment: .leading, spacing: 22) {
                        greeting
                        connectionCard
                        if session.isDevelopment {
                            Callout(
                                tone: .info,
                                text: "Development cluster: mock attestation is trusted for this node only."
                            )
                            .accessibilityIdentifier("sessionNote")
                        }
                        CalSection(title: "Try it") {
                            Button {
                                tab = .chat
                            } label: {
                                CalRow(
                                    icon: "bubble.left.and.bubble.right.fill", title: "Chat",
                                    subtitle: "Orgs, teams and live messages", accent: true)
                            }
                            .buttonStyle(.plain)
                            .accessibilityIdentifier("openChat")
                            RowDivider()
                            NavigationLink {
                                SDKListView()
                            } label: {
                                CalRow(
                                    icon: "square.grid.2x2.fill", title: "Explore SDK",
                                    subtitle: "\(sdkOperations.count) methods in \(sdkCategories.count) categories")
                            }
                            .buttonStyle(.plain)
                            .accessibilityIdentifier("exploreSDK")
                        }
                        CalSection(title: "Session") {
                            Button {
                                showLogs = true
                            } label: {
                                CalRow(
                                    icon: "waveform.path.ecg", title: "Diagnostics", subtitle: "Every request and event"
                                )
                            }
                            .buttonStyle(.plain)
                            RowDivider()
                            Button {
                                Task { await session.logout() }
                            } label: {
                                HStack(spacing: 12) {
                                    Image(systemName: "rectangle.portrait.and.arrow.right")
                                        .font(.system(size: 16))
                                        .foregroundColor(Cal.error)
                                        .frame(width: 36, height: 36)
                                        .background(RoundedRectangle(cornerRadius: 11).fill(Cal.errorSoft))
                                    Text("Sign out").font(.subheadline.weight(.semibold)).foregroundColor(Cal.error)
                                    Spacer()
                                }
                                .padding(.horizontal, 14)
                                .frame(minHeight: 60)
                                .contentShape(Rectangle())
                            }
                            .buttonStyle(.plain)
                            .accessibilityIdentifier("logoutButton")
                        }
                    }
                    .padding(.horizontal, Cal.screenPad + 4)
                    .padding(.vertical, 12)
                }
            }
            .toolbar(.hidden, for: .navigationBar)
        }
        .sheet(isPresented: $showLogs) { LogsView() }
    }

    private var greeting: some View {
        HStack(alignment: .center, spacing: 14) {
            ChatAvatar(name: session.displayName.isEmpty ? "?" : session.displayName, size: 52)
            VStack(alignment: .leading, spacing: 3) {
                Text("Signed in as")
                    .font(.footnote)
                    .foregroundColor(Cal.textFaint)
                Text(session.displayName)
                    .font(.system(.title2, design: .rounded).weight(.bold))
                    .foregroundColor(Cal.text)
                    .lineLimit(1)
                    .accessibilityIdentifier("homeUser")
            }
            Spacer()
            CalLogo(size: 34, showWordmark: false)
        }
        .padding(.top, 8)
    }

    private var connectionCard: some View {
        CalCard {
            VStack(alignment: .leading, spacing: 14) {
                HStack(spacing: 12) {
                    IconTile(
                        systemName: session.isDevelopment ? "server.rack" : "cloud.fill", accent: true, size: 40)
                    VStack(alignment: .leading, spacing: 3) {
                        Text(session.isDevelopment ? "Development cluster" : "Calimero Cloud")
                            .font(.subheadline.weight(.semibold))
                            .foregroundColor(Cal.text)
                        Text(statusText).font(.footnote).foregroundColor(Cal.textFaint).lineLimit(1)
                    }
                    Spacer()
                    HStack(spacing: 6) {
                        Circle().fill(isLive ? Cal.lime : Cal.warning).frame(width: 8, height: 8)
                        Text(isLive ? "Live" : "Limited")
                            .font(.caption.weight(.semibold))
                            .foregroundColor(isLive ? Cal.accentInk : Cal.warning)
                    }
                    .padding(.horizontal, 10)
                    .padding(.vertical, 5)
                    .background(Capsule().fill(isLive ? Cal.accentSoft : Cal.warningSoft))
                }
                TechnicalDetails(rows: technicalRows)
            }
        }
        .accessibilityIdentifier("accountCard")
    }

    private var isLive: Bool { session.isAuthenticated }

    private var statusText: String {
        guard let node = session.nodeURL else { return "Not connected" }
        return URL(string: node).flatMap { hostPort($0) } ?? node
    }

    private func hostPort(_ url: URL) -> String? {
        guard let host = url.host else { return nil }
        return url.port.map { "\(host):\($0)" } ?? host
    }

    private var technicalRows: [(String, String)] {
        var rows: [(String, String)] = []
        if let account = session.account { rows.append(("Account", account)) }
        if let node = session.nodeURL { rows.append(("Node", node)) }
        rows.append(("Attestation", session.isDevelopment ? "mock (development)" : "TDX, signed mero-tee release"))
        rows.append(("Channel", "calimero/sealed-http/v2"))
        return rows
    }
}

// MARK: - SDK list (categorized, searchable) — behind "Explore SDK"

struct SDKListView: View {
    @State private var search = ""
    @State private var expanded: Set<String> = []

    private var filtered: [(category: String, ops: [SDKOperation])] {
        let q = search.trimmingCharacters(in: .whitespaces).lowercased()
        return sdkCategories.compactMap { cat in
            let ops = sdkOperations.filter { op in
                op.category == cat
                    && (q.isEmpty || op.name.lowercased().contains(q) || op.summary.lowercased().contains(q)
                        || op.category.lowercased().contains(q))
            }
            return ops.isEmpty ? nil : (cat, ops)
        }
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 8) {
                searchField
                sectionLabel(search.isEmpty ? "SDK Options" : "\(matchCount) results")
                ForEach(filtered, id: \.category) { section in
                    categoryCard(section)
                }
            }
            .padding(.horizontal, Cal.screenPad)
            .padding(.vertical, 14)
        }
        .background(Cal.bg)
        .navigationTitle("Explore SDK")
        .navigationBarTitleDisplayMode(.inline)
    }

    // A plain, always-visible search field (instead of `.searchable`, whose bar
    // collapses on a pushed screen and isn't reliably reachable in UI tests).
    private var searchField: some View {
        HStack(spacing: 8) {
            Image(systemName: "magnifyingglass").font(.footnote).foregroundColor(Cal.textDim)
            TextField("Search \(sdkOperations.count) methods", text: $search)
                .font(.subheadline)
                .foregroundColor(Cal.text)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
                .accessibilityIdentifier("sdkSearch")
            if !search.isEmpty {
                Button {
                    search = ""
                } label: {
                    Image(systemName: "xmark.circle.fill").foregroundColor(Cal.textDim)
                }
            }
        }
        .padding(.horizontal, 12).padding(.vertical, 10)
        .background(Cal.surface2)
        .overlay(RoundedRectangle(cornerRadius: Cal.controlRadius).stroke(Cal.borderStrong, lineWidth: 1))
        .clipShape(RoundedRectangle(cornerRadius: Cal.controlRadius))
    }

    private func sectionLabel(_ text: String) -> some View {
        Text(text.uppercased())
            .font(.caption.weight(.medium))
            .tracking(0.5)
            .foregroundColor(Cal.textFaint)
    }

    private var matchCount: Int { filtered.reduce(0) { $0 + $1.ops.count } }

    private func expandBinding(_ category: String) -> Binding<Bool> {
        Binding(
            get: { !search.isEmpty || expanded.contains(category) },
            set: { isOn in
                if isOn { expanded.insert(category) } else { expanded.remove(category) }
            })
    }

    private func categoryCard(_ section: (category: String, ops: [SDKOperation])) -> some View {
        DisclosureGroup(isExpanded: expandBinding(section.category)) {
            VStack(spacing: 0) {
                ForEach(Array(section.ops.enumerated()), id: \.element.id) { idx, op in
                    NavigationLink {
                        OperationRunnerView(op: op)
                    } label: {
                        row(op)
                    }
                    if idx < section.ops.count - 1 { Divider().overlay(Cal.border) }
                }
            }
            .padding(.top, 6)
        } label: {
            HStack {
                Text(section.category).font(.subheadline.weight(.semibold)).foregroundColor(Cal.text)
                Spacer()
                Text("\(section.ops.count)").font(.caption).foregroundColor(Cal.textDim)
            }
        }
        .padding(12)
        .background(Cal.surface)
        .overlay(RoundedRectangle(cornerRadius: Cal.cardRadius).stroke(Cal.border, lineWidth: 1))
        .clipShape(RoundedRectangle(cornerRadius: Cal.cardRadius))
        .tint(Cal.accentInk)
    }

    private func row(_ op: SDKOperation) -> some View {
        HStack {
            VStack(alignment: .leading, spacing: 2) {
                Text(op.name).font(.subheadline.weight(.semibold)).foregroundColor(Cal.text)
                Text(op.summary).font(.caption).foregroundColor(Cal.textDim)
            }
            Spacer()
            Image(systemName: "chevron.right").font(.caption).foregroundColor(Cal.textDim)
        }
        .padding(.horizontal, 14).padding(.vertical, 11)
        .contentShape(Rectangle())
    }
}

// MARK: - Operation runner (form + response)

struct OperationRunnerView: View {
    let op: SDKOperation
    @EnvironmentObject private var session: MeroSession
    @State private var inputs: [String: String] = [:]
    @State private var output = ""
    @State private var failed = false
    @State private var running = false

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                VStack(alignment: .leading, spacing: 4) {
                    Text(op.name).font(.title3.bold()).foregroundColor(Cal.text)
                    Text(op.summary).font(.subheadline).foregroundColor(Cal.textDim)
                    Text(op.category).font(.caption.weight(.medium)).foregroundColor(Cal.accentInk)
                }

                ForEach(op.fields) { field in
                    fieldView(field)
                }

                Button {
                    run()
                } label: {
                    if running { ProgressView().tint(Cal.text) } else { Text("Run") }
                }
                .buttonStyle(CalPrimaryButtonStyle())
                .disabled(running)

                if !output.isEmpty {
                    VStack(alignment: .leading, spacing: 6) {
                        Eyebrow(text: failed ? "Error" : "Response")
                        ScrollView(.horizontal, showsIndicators: true) {
                            Text(output)
                                .font(Cal.mono)
                                .foregroundColor(failed ? Cal.error : Cal.text)
                                .textSelection(.enabled)
                                .frame(maxWidth: .infinity, alignment: .leading)
                        }
                        .padding(12)
                        .background(Cal.surface2)
                        .overlay(
                            RoundedRectangle(cornerRadius: Cal.controlRadius).stroke(Cal.borderStrong, lineWidth: 1)
                        )
                        .clipShape(RoundedRectangle(cornerRadius: Cal.controlRadius))
                    }
                }
            }
            .padding(.horizontal, Cal.screenPad)
            .padding(.vertical, 16)
        }
        .background(Cal.bg.ignoresSafeArea())
        .navigationTitle(op.name)
        .navigationBarTitleDisplayMode(.inline)
    }

    @ViewBuilder
    private func fieldView(_ field: OpField) -> some View {
        let binding = Binding(get: { inputs[field.id] ?? "" }, set: { inputs[field.id] = $0 })
        switch field.kind {
        case .line:
            CalField(title: field.label, text: binding, placeholder: field.placeholder)
        case .multiline:
            VStack(alignment: .leading, spacing: 6) {
                Text(field.label).font(.footnote.weight(.medium)).foregroundColor(Cal.text)
                TextEditor(text: binding)
                    .font(Cal.mono)
                    .foregroundColor(Cal.text)
                    .scrollContentBackground(.hidden)
                    .frame(minHeight: 120)
                    .padding(8)
                    .background(Cal.surface2)
                    .overlay(RoundedRectangle(cornerRadius: Cal.controlRadius).stroke(Cal.borderStrong, lineWidth: 1))
                    .clipShape(RoundedRectangle(cornerRadius: Cal.controlRadius))
            }
        }
    }

    private func run() {
        running = true
        let captured = inputs
        let cloud = session.cloud
        let passkey = session.client.passkeyForNode()
        Task {
            do {
                let result: String
                if let cloud {
                    result = try await op.run(cloud, captured, passkey)
                } else {
                    result = "Sign in first."
                }
                await MainActor.run {
                    output = result; failed = false; running = false
                }
            } catch {
                await MainActor.run {
                    output = MeroClient.describe(error)
                    failed = true; running = false
                }
            }
        }
    }
}

// MARK: - Diagnostics log

struct LogsView: View {
    @EnvironmentObject private var session: MeroSession
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            ScrollViewReader { proxy in
                ScrollView {
                    VStack(alignment: .leading, spacing: 5) {
                        if session.logs.isEmpty {
                            Text("No activity yet.")
                                .font(.footnote).foregroundColor(Cal.textDim)
                        }
                        ForEach(session.logs) { line in
                            HStack(alignment: .top, spacing: 8) {
                                Image(systemName: line.level.symbol)
                                    .font(.caption2)
                                    .foregroundColor(color(line.level))
                                    .frame(width: 14, alignment: .leading)
                                Text(line.text)
                                    .foregroundColor(Cal.text)
                                    .textSelection(.enabled)
                                    .frame(maxWidth: .infinity, alignment: .leading)
                            }
                            .font(Cal.mono)
                            .id(line.id)
                        }
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(16)
                }
                .onChange(of: session.logs.count) { _ in
                    if let last = session.logs.last { withAnimation { proxy.scrollTo(last.id, anchor: .bottom) } }
                }
            }
            .background(Cal.bg)
            .navigationTitle("Diagnostics")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) {
                    Button("Clear") { session.clearLogs() }.foregroundColor(Cal.accentInk)
                }
                ToolbarItem(placement: .topBarTrailing) {
                    HStack(spacing: 16) {
                        Button {
                            UIPasteboard.general.string = session.logText()
                        } label: {
                            Image(systemName: "doc.on.doc")
                        }
                        ShareLink(item: session.logText()) { Image(systemName: "square.and.arrow.up") }
                        Button("Done") { dismiss() }
                    }
                    .foregroundColor(Cal.accentInk)
                }
            }
        }
        .tint(Cal.accentInk)
    }

    private func color(_ level: MeroSession.LogLine.Level) -> Color {
        switch level {
        case .err: return Cal.error
        case .ok: return Cal.success
        case .warn: return Cal.warning
        case .req: return Cal.text
        case .info: return Cal.textDim
        }
    }
}
