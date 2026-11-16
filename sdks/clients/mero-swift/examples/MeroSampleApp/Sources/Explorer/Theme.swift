import SwiftUI
import UIKit

/// Calimero light design tokens (the mero-vote / mero-forum light redesign).
///
/// Lime is a FILL colour (buttons, brand mark, live dot) with ink text on it;
/// it fails contrast as text on white, so lime-family text uses `accentInk`.
/// The system font is used throughout (Power Grotesk is not licensed for
/// embedding in an app).
enum Cal {
    /// Page background (warm off-white).
    static let bg = Color(hex: 0xF6F6F3)
    /// Icon tiles, default badges, avatar default.
    static let bgSubtle = Color(hex: 0xEFEFEB)
    /// Cards, top bar, inputs.
    static let surface = Color.white
    /// Pressed rows / ghost buttons.
    static let surfaceHover = Color(hex: 0xF3F3F0)
    /// Id fields, code blocks, disabled inputs.
    static let surface2 = Color(hex: 0xF1F1EE)
    /// 1px hairline on cards and dividers.
    static let border = Color(hex: 0xE5E5E0)
    /// Input and secondary-button borders.
    static let borderStrong = Color(hex: 0xD4D4CE)
    /// Ink: primary text, and text on lime.
    static let text = Color(hex: 0x131215)
    static let textDim = Color(hex: 0x4A4A4F)
    static let textFaint = Color(hex: 0x6B6B70)
    /// Lime — fills only.
    static let lime = Color(hex: 0xA5FF11)
    static let limePressed = Color(hex: 0xB4FF3A)
    static let accentSoft = Color(hex: 0xF0FFD6)
    /// Lime-family text on white.
    static let accentInk = Color(hex: 0x4A7300)
    static let error = Color(hex: 0xC62828)
    static let errorSoft = Color(hex: 0xFDECEC)
    static let warning = Color(hex: 0x9A5B00)
    static let warningSoft = Color(hex: 0xFFF4E0)
    static let info = Color(hex: 0x1D5FBF)
    static let success = Color(hex: 0x2F7A00)
    static let mono = Font.system(.footnote, design: .monospaced)

    /// Screen gutter.
    static let screenPad: CGFloat = 16
    /// Buttons and inputs.
    static let controlRadius: CGFloat = 12
    /// Cards.
    static let cardRadius: CGFloat = 18
}

extension Color {
    init(hex: UInt) {
        self.init(
            .sRGB,
            red: Double((hex >> 16) & 0xFF) / 255,
            green: Double((hex >> 8) & 0xFF) / 255,
            blue: Double(hex & 0xFF) / 255,
            opacity: 1
        )
    }
}

/// Lime-filled primary button with ink text.
struct CalPrimaryButtonStyle: ButtonStyle {
    var enabled = true
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.subheadline.weight(.semibold))
            .foregroundColor(Cal.text)
            .frame(maxWidth: .infinity, minHeight: 52)
            .background(configuration.isPressed ? Cal.limePressed : Cal.lime)
            .overlay(
                RoundedRectangle(cornerRadius: Cal.controlRadius).stroke(Color.black.opacity(0.06), lineWidth: 1)
            )
            .clipShape(RoundedRectangle(cornerRadius: Cal.controlRadius))
            .opacity(enabled ? 1 : 0.5)
    }
}

/// White secondary button with a strong border.
struct CalSecondaryButtonStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.subheadline.weight(.medium))
            .foregroundColor(Cal.text)
            .frame(maxWidth: .infinity, minHeight: 48)
            .background(configuration.isPressed ? Cal.surfaceHover : Cal.surface)
            .overlay(RoundedRectangle(cornerRadius: Cal.controlRadius).stroke(Cal.borderStrong, lineWidth: 1))
            .clipShape(RoundedRectangle(cornerRadius: Cal.controlRadius))
    }
}

/// A white hairline card.
struct CalCard<Content: View>: View {
    var padding: CGFloat = 16
    @ViewBuilder var content: Content
    var body: some View {
        content
            .padding(padding)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Cal.surface)
            .overlay(RoundedRectangle(cornerRadius: Cal.cardRadius).stroke(Cal.border, lineWidth: 1))
            .clipShape(RoundedRectangle(cornerRadius: Cal.cardRadius))
            .shadow(color: Cal.text.opacity(0.04), radius: 12, y: 4)
    }
}

/// A 34pt rounded icon tile (accent: lime-soft with accent-ink glyph).
struct IconTile: View {
    let systemName: String
    var accent = false
    var size: CGFloat = 34
    var body: some View {
        Image(systemName: systemName)
            .font(.system(size: size * 0.45, weight: .regular))
            .foregroundColor(accent ? Cal.accentInk : Cal.textDim)
            .frame(width: size, height: size)
            .background(RoundedRectangle(cornerRadius: size * 0.3).fill(accent ? Cal.accentSoft : Cal.bgSubtle))
            .accessibilityHidden(true)
    }
}

/// Uppercase section label ("eyebrow").
struct Eyebrow: View {
    let text: String
    var body: some View {
        Text(text.uppercased())
            .font(.caption.weight(.medium))
            .tracking(0.5)
            .foregroundColor(Cal.textFaint)
    }
}

/// The Calimero brand mark: the icon on a lime square, plus the app name.
struct CalLogo: View {
    var size: CGFloat = 24
    var showWordmark = true
    var body: some View {
        HStack(spacing: 8) {
            Image("CalimeroIcon")
                .resizable()
                .scaledToFit()
                .padding(size * 0.18)
                .frame(width: size, height: size)
                .background(RoundedRectangle(cornerRadius: 8).fill(Cal.lime))
                .overlay(RoundedRectangle(cornerRadius: 8).stroke(Color.black.opacity(0.06), lineWidth: 1))
            if showWordmark {
                Text("MeroKit")
                    .font(.system(size: size * 0.7, weight: .bold))
                    .foregroundColor(Cal.text)
            }
        }
    }
}

/// A labelled single-line text field.
struct CalField: View {
    let title: String
    @Binding var text: String
    var placeholder: String = ""
    var secure = false
    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(title)
                .font(.footnote.weight(.medium))
                .foregroundColor(Cal.text)
            Group {
                if secure { SecureField(placeholder, text: $text) } else { TextField(placeholder, text: $text) }
            }
            .font(.subheadline)
            .textInputAutocapitalization(.never)
            .disableAutocorrection(true)
            .foregroundColor(Cal.text)
            .padding(.horizontal, 14)
            .frame(minHeight: 48)
            .background(Cal.surface)
            .overlay(RoundedRectangle(cornerRadius: Cal.controlRadius).stroke(Cal.borderStrong, lineWidth: 1))
            .clipShape(RoundedRectangle(cornerRadius: Cal.controlRadius))
        }
    }
}

/// A monospaced, ellipsized id with a copy button.
struct IdField: View {
    let label: String
    let value: String
    var body: some View {
        HStack(spacing: 8) {
            Text(label)
                .font(.footnote)
                .foregroundColor(Cal.textFaint)
                .frame(width: 84, alignment: .leading)
            Text(value)
                .font(Cal.mono)
                .foregroundColor(Cal.textDim)
                .lineLimit(1)
                .truncationMode(.middle)
                .frame(maxWidth: .infinity, alignment: .leading)
            Button {
                UIPasteboard.general.string = value
            } label: {
                Image(systemName: "doc.on.doc").font(.footnote)
            }
            .foregroundColor(Cal.textDim)
            .accessibilityLabel("Copy \(label)")
        }
        .padding(.horizontal, 8)
        .frame(minHeight: 30)
        .background(RoundedRectangle(cornerRadius: 6).fill(Cal.surface2))
        .overlay(RoundedRectangle(cornerRadius: 6).stroke(Cal.border, lineWidth: 1))
    }
}

/// "Show technical details": ids live behind this, never up front.
struct TechnicalDetails: View {
    let rows: [(String, String)]
    @State private var open = false
    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Divider().overlay(Cal.border)
            Button {
                withAnimation(.easeInOut(duration: 0.15)) { open.toggle() }
            } label: {
                HStack(spacing: 6) {
                    Image(systemName: "chevron.right")
                        .font(.caption.weight(.semibold))
                        .rotationEffect(.degrees(open ? 90 : 0))
                    Text("Show technical details").font(.footnote.weight(.medium))
                }
                .foregroundColor(Cal.textFaint)
            }
            .buttonStyle(.plain)
            .accessibilityIdentifier("technicalDetails")
            if open {
                ForEach(rows, id: \.0) { IdField(label: $0.0, value: $0.1) }
            }
        }
    }
}

/// A soft, tinted callout with a leading icon.
struct Callout: View {
    enum Tone { case info, warning, danger, success }
    let tone: Tone
    let text: String
    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: icon).foregroundColor(fg)
            Text(text).font(.footnote).foregroundColor(Cal.textDim).fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 0)
        }
        .padding(12)
        .background(RoundedRectangle(cornerRadius: 10).fill(bg))
        .overlay(RoundedRectangle(cornerRadius: 10).stroke(fg.opacity(0.18), lineWidth: 1))
    }
    private var icon: String {
        switch tone {
        case .info: return "info.circle"
        case .warning: return "exclamationmark.triangle"
        case .danger: return "exclamationmark.circle"
        case .success: return "checkmark.circle"
        }
    }
    private var fg: Color {
        switch tone {
        case .info: return Cal.info
        case .warning: return Cal.warning
        case .danger: return Cal.error
        case .success: return Cal.success
        }
    }
    private var bg: Color {
        switch tone {
        case .info: return Color(hex: 0xEAF1FC)
        case .warning: return Cal.warningSoft
        case .danger: return Cal.errorSoft
        case .success: return Color(hex: 0xEEF8E4)
        }
    }
}

/// A borderless text button for secondary actions ("Manually enter node URL").
struct CalTextButtonStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.footnote.weight(.medium))
            .foregroundColor(Cal.textDim)
            .padding(.vertical, 8)
            .opacity(configuration.isPressed ? 0.6 : 1)
    }
}

/// A soft lime glow behind the top of a screen.
struct LimeGlow: View {
    var body: some View {
        RadialGradient(
            colors: [Cal.lime.opacity(0.35), Cal.lime.opacity(0.08), .clear],
            center: .top, startRadius: 0, endRadius: 420
        )
        .ignoresSafeArea()
        .accessibilityHidden(true)
    }
}

/// A grouped list section: an eyebrow over a card of divided rows.
struct CalSection<Content: View>: View {
    let title: String
    @ViewBuilder var content: Content
    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Eyebrow(text: title).padding(.leading, 4)
            VStack(spacing: 0) { content }
                .background(Cal.surface)
                .overlay(RoundedRectangle(cornerRadius: Cal.cardRadius).stroke(Cal.border, lineWidth: 1))
                .clipShape(RoundedRectangle(cornerRadius: Cal.cardRadius))
        }
    }
}

/// One row in a ``CalSection``: icon tile, title, optional subtitle, chevron.
struct CalRow: View {
    let icon: String
    let title: String
    var subtitle: String?
    var accent = false
    var chevron = true
    var body: some View {
        HStack(spacing: 12) {
            IconTile(systemName: icon, accent: accent, size: 36)
            VStack(alignment: .leading, spacing: 2) {
                Text(title).font(.subheadline.weight(.semibold)).foregroundColor(Cal.text)
                if let subtitle {
                    Text(subtitle).font(.footnote).foregroundColor(Cal.textFaint).lineLimit(1)
                }
            }
            Spacer(minLength: 8)
            if chevron {
                Image(systemName: "chevron.right").font(.footnote.weight(.semibold)).foregroundColor(Cal.textFaint)
            }
        }
        .padding(.horizontal, 14)
        .frame(minHeight: 60)
        .contentShape(Rectangle())
    }
}

/// A hairline between rows of a ``CalSection``, inset past the icon.
struct RowDivider: View {
    var body: some View { Divider().overlay(Cal.border).padding(.leading, 62) }
}
