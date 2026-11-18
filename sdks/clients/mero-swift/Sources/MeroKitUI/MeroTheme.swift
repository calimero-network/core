#if canImport(SwiftUI)
import SwiftUI

/// Theme tokens for the MeroKitUI views — mero-react's `MeroTheme`, as SwiftUI
/// values. Apply one with ``SwiftUI/View/meroTheme(_:)``; every MeroKitUI view
/// reads it from the environment.
///
/// Colours, radii and type for every MeroKitUI view. ``LoginView`` draws its
/// whole screen from it (background, surfaces, text, accent); the other views
/// use the accent, its text, the error and the radius.
///
/// Presets: ``calimeroLight`` (the default in the environment) and
/// ``calimero`` (dark, mero-react's palette). Override any token with
/// ``with(_:)``:
///
/// ```swift
/// LoginView(callbackScheme: "myapp", mode: .cloudOnly)
///     .meroTheme(.calimeroLight.with { $0.primary = .indigo; $0.primaryText = .white })
/// ```
public struct MeroTheme: Equatable {
    /// Primary accent / call-to-action.
    public var primary: Color
    /// Primary accent, pressed.
    public var primaryHover: Color
    /// Text drawn on ``primary``.
    public var primaryText: Color
    /// Main surface.
    public var background: Color
    /// Secondary surface (inputs, chips).
    public var backgroundSecondary: Color
    /// Tertiary surface (subtle elevation).
    public var backgroundTertiary: Color
    public var border: Color
    public var text: Color
    /// Muted text.
    public var textSecondary: Color
    /// Errors and danger.
    public var error: Color
    /// A modal's backdrop.
    public var overlay: Color
    /// Corner radius, in points.
    public var radius: CGFloat
    /// Cards and panels (the node form on ``LoginView``).
    public var surface: Color
    /// Link-style and tertiary text.
    public var textTertiary: Color
    public var success: Color
    public var warning: Color
    /// Corner radius of cards, in points.
    public var cardRadius: CGFloat
    /// Height of buttons and text fields, in points.
    public var controlHeight: CGFloat
    /// The font design of titles (`.rounded`, `.default`, `.serif`, `.monospaced`).
    public var titleDesign: Font.Design
    /// A soft glow of ``primary`` behind the top of ``LoginView``.
    public var showsGlow: Bool

    public init(
        primary: Color, primaryHover: Color, primaryText: Color, background: Color, backgroundSecondary: Color,
        backgroundTertiary: Color, border: Color, text: Color, textSecondary: Color, error: Color, overlay: Color,
        radius: CGFloat, surface: Color? = nil, textTertiary: Color? = nil, success: Color = Color(meroHex: 0x2F7A00),
        warning: Color = Color(meroHex: 0x9A5B00), cardRadius: CGFloat? = nil, controlHeight: CGFloat = 52,
        titleDesign: Font.Design = .rounded, showsGlow: Bool = true
    ) {
        self.surface = surface ?? backgroundSecondary
        self.textTertiary = textTertiary ?? textSecondary
        self.success = success
        self.warning = warning
        self.cardRadius = cardRadius ?? radius * 2
        self.controlHeight = controlHeight
        self.titleDesign = titleDesign
        self.showsGlow = showsGlow
        self.primary = primary
        self.primaryHover = primaryHover
        self.primaryText = primaryText
        self.background = background
        self.backgroundSecondary = backgroundSecondary
        self.backgroundTertiary = backgroundTertiary
        self.border = border
        self.text = text
        self.textSecondary = textSecondary
        self.error = error
        self.overlay = overlay
        self.radius = radius
    }

    /// mero-react's `defaultMeroTheme`: the green palette across Calimero apps,
    /// accent `#A5FF11`.
    public static var calimero: MeroTheme {
        MeroTheme(
            primary: Color(meroHex: 0xA5FF11), primaryHover: Color(meroHex: 0x8ED40D),
            primaryText: Color(meroHex: 0x0D1117), background: Color(meroHex: 0x0D1117),
            backgroundSecondary: Color(meroHex: 0x161B22), backgroundTertiary: Color(meroHex: 0x1C2128),
            border: Color(meroHex: 0x30363D), text: Color(meroHex: 0xE6EDF3), textSecondary: Color(meroHex: 0x8B949E),
            error: Color(meroHex: 0xFF6B6B), overlay: Color.black.opacity(0.75), radius: 8,
            textTertiary: Color(meroHex: 0x6E7681), success: Color(meroHex: 0x56D364),
            warning: Color(meroHex: 0xE3B341), cardRadius: 18)
    }

    /// The light Calimero design the sample apps use: warm off-white, white
    /// cards, ink text, lime fills. The environment's default.
    public static var calimeroLight: MeroTheme {
        MeroTheme(
            primary: Color(meroHex: 0xA5FF11), primaryHover: Color(meroHex: 0xB4FF3A),
            primaryText: Color(meroHex: 0x131215), background: Color(meroHex: 0xF6F6F3),
            backgroundSecondary: Color(meroHex: 0xF1F1EE), backgroundTertiary: Color(meroHex: 0xEFEFEB),
            border: Color(meroHex: 0xE5E5E0), text: Color(meroHex: 0x131215), textSecondary: Color(meroHex: 0x4A4A4F),
            error: Color(meroHex: 0xC62828), overlay: Color.black.opacity(0.32), radius: 12, surface: .white,
            textTertiary: Color(meroHex: 0x6B6B70), success: Color(meroHex: 0x2F7A00),
            warning: Color(meroHex: 0x9A5B00), cardRadius: 18)
    }

    /// A copy with `change` applied — the partial override mero-react's
    /// `resolveMeroTheme` merges.
    public func with(_ change: (inout MeroTheme) -> Void) -> MeroTheme {
        var copy = self
        change(&copy)
        return copy
    }
}

/// mero-react's default theme (dark). The SwiftUI environment defaults to
/// ``MeroTheme/calimeroLight`` instead; apply this one with `.meroTheme(defaultMeroTheme)`.
public var defaultMeroTheme: MeroTheme { .calimero }

extension Color {
    /// A colour from `0xRRGGBB`.
    public init(meroHex rgb: UInt32, opacity: Double = 1) {
        self.init(
            .sRGB, red: Double((rgb >> 16) & 0xFF) / 255, green: Double((rgb >> 8) & 0xFF) / 255,
            blue: Double(rgb & 0xFF) / 255, opacity: opacity)
    }
}

private struct MeroThemeKey: EnvironmentKey {
    static var defaultValue: MeroTheme { .calimeroLight }
}

extension EnvironmentValues {
    /// The ``MeroTheme`` MeroKitUI views draw with.
    public var meroTheme: MeroTheme {
        get { self[MeroThemeKey.self] }
        set { self[MeroThemeKey.self] = newValue }
    }
}

extension View {
    /// Draw MeroKitUI views below with `theme`.
    public func meroTheme(_ theme: MeroTheme) -> some View {
        environment(\.meroTheme, theme)
    }
}
#endif
