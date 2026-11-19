package com.calimero.mero.compose

import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.ProvidableCompositionLocal
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp

/**
 * Theme tokens for the mero-compose components — mero-react `MeroTheme`, with the same fields,
 * plus the ones a sign-in screen draws from (== the Swift SDK's `MeroTheme`).
 *
 * Provide one with [ProvideMeroTheme]; components read [LocalMeroTheme]. Presets:
 * [MeroTheme.calimeroLight] (the default) and [MeroTheme.calimero] (dark, mero-react's palette).
 * Override any token with `copy(...)`:
 *
 * ```kotlin
 * ProvideMeroTheme(MeroTheme.calimeroLight.copy(primary = Color(0xFF5B5BD6), primaryText = Color.White)) {
 *     MyScreen()
 * }
 * ```
 */
@Immutable
data class MeroTheme(
    /** Primary accent / call to action. */
    val primary: Color,
    val primaryHover: Color,
    /** Text drawn on [primary]. */
    val primaryText: Color,
    /** The main surface (sheets, dialogs, a sign-in screen). */
    val background: Color,
    /** The secondary surface (fields, chips). */
    val backgroundSecondary: Color,
    /** Subtle elevation. */
    val backgroundTertiary: Color,
    val border: Color,
    val text: Color,
    /** Muted / secondary text. */
    val textSecondary: Color,
    val error: Color,
    /** A dialog's scrim. */
    val overlay: Color,
    /** Corner radius of buttons and fields. */
    val radius: Dp,
    /** Cards and panels (a sign-in form). */
    val surface: Color = backgroundSecondary,
    /** Link-style and tertiary text. */
    val textTertiary: Color = textSecondary,
    val success: Color = Color(0xFF2F7A00),
    val warning: Color = Color(0xFF9A5B00),
    /** Corner radius of cards. */
    val cardRadius: Dp = radius * 2,
    /** Height of buttons and text fields. */
    val controlHeight: Dp = 52.dp,
    /** The font family of titles (Swift's `titleDesign`). */
    val titleFontFamily: FontFamily = FontFamily.Default,
    /** A soft glow of [primary] behind the top of a sign-in screen. */
    val showsGlow: Boolean = true,
) {
    companion object {
        /** mero-react's `defaultMeroTheme`: the dark green palette of Calimero's apps (accent #A5FF11). */
        val calimero: MeroTheme =
            MeroTheme(
                primary = Color(0xFFA5FF11),
                primaryHover = Color(0xFF8ED40D),
                primaryText = Color(0xFF0D1117),
                background = Color(0xFF0D1117),
                backgroundSecondary = Color(0xFF161B22),
                backgroundTertiary = Color(0xFF1C2128),
                border = Color(0xFF30363D),
                text = Color(0xFFE6EDF3),
                textSecondary = Color(0xFF8B949E),
                error = Color(0xFFFF6B6B),
                overlay = Color(0xBF000000),
                radius = 8.dp,
                textTertiary = Color(0xFF6E7681),
                success = Color(0xFF56D364),
                warning = Color(0xFFE3B341),
                cardRadius = 18.dp,
            )

        /** The light Calimero design the sample apps use: warm off-white, white cards, ink text, lime fills. The default. */
        val calimeroLight: MeroTheme =
            MeroTheme(
                primary = Color(0xFFA5FF11),
                primaryHover = Color(0xFFB4FF3A),
                primaryText = Color(0xFF131215),
                background = Color(0xFFF6F6F3),
                backgroundSecondary = Color(0xFFF1F1EE),
                backgroundTertiary = Color(0xFFEFEFEB),
                border = Color(0xFFE5E5E0),
                text = Color(0xFF131215),
                textSecondary = Color(0xFF4A4A4F),
                error = Color(0xFFC62828),
                overlay = Color(0x52000000),
                radius = 12.dp,
                surface = Color(0xFFFFFFFF),
                textTertiary = Color(0xFF6B6B70),
                success = Color(0xFF2F7A00),
                warning = Color(0xFF9A5B00),
                cardRadius = 18.dp,
            )
    }
}

/** mero-react's `defaultMeroTheme` (dark): [MeroTheme.calimero]. */
val defaultMeroTheme: MeroTheme = MeroTheme.calimero

/** The light Calimero tokens (white sheet). See also [MeroTheme.calimeroLight]. */
val lightMeroTheme: MeroTheme =
    MeroTheme(
        primary = Color(0xFFA5FF11),
        primaryHover = Color(0xFF8ED40D),
        primaryText = Color(0xFF131215),
        background = Color(0xFFFFFFFF),
        backgroundSecondary = Color(0xFFF7F7F4),
        backgroundTertiary = Color(0xFFEFEFEA),
        border = Color(0xFFE5E5E0),
        text = Color(0xFF131215),
        textSecondary = Color(0xFF4A4A4F),
        error = Color(0xFFC62828),
        overlay = Color(0x66000000),
        radius = 8.dp,
        textTertiary = Color(0xFF6B6B70),
    )

/** The ambient [MeroTheme]; [MeroTheme.calimeroLight] when none is provided (as in the Swift SDK). */
val LocalMeroTheme: ProvidableCompositionLocal<MeroTheme> = staticCompositionLocalOf { MeroTheme.calimeroLight }

/** Give [content] a [MeroTheme]. */
@Composable
fun ProvideMeroTheme(
    theme: MeroTheme,
    content: @Composable () -> Unit,
) {
    CompositionLocalProvider(LocalMeroTheme provides theme, content = content)
}
