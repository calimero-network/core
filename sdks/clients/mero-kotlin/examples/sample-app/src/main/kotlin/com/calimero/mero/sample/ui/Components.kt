@file:Suppress("TooManyFunctions")

package com.calimero.mero.sample.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.ChevronRight
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.calimero.mero.compose.CalimeroLogoVector

/** A soft lime glow behind the top of a screen. */
@Composable
fun LimeGlow(
    modifier: Modifier = Modifier,
    height: Dp = 420.dp,
) {
    Box(
        modifier
            .fillMaxWidth()
            .heightIn(min = height)
            .drawBehind {
                if (size.width <= 0f || size.height <= 0f) return@drawBehind
                drawRect(
                    Brush.radialGradient(
                        colorStops =
                            arrayOf(
                                0f to Cal.lime.copy(alpha = 0.35f),
                                GLOW_MID to Cal.lime.copy(alpha = 0.08f),
                                1f to Color.Transparent,
                            ),
                        center = Offset(size.width / 2f, 0f),
                        radius = maxOf(size.width, size.height) * GLOW_REACH,
                    ),
                )
            },
    )
}

private const val GLOW_MID = 0.45f
private const val GLOW_REACH = 1.1f

/** The Calimero emblem on a lime tile (the sign-in hero and the home header). */
@Composable
fun CalIconTile(size: Dp = 76.dp) {
    Box(
        Modifier
            .size(size)
            .clip(RoundedCornerShape(size * 0.29f))
            .background(Cal.lime)
            .border(1.dp, Cal.limeEdge, RoundedCornerShape(size * 0.29f)),
        contentAlignment = Alignment.Center,
    ) {
        Icon(CalimeroLogoVector, contentDescription = null, tint = Cal.onLime, modifier = Modifier.size(size * 0.55f))
    }
}

/** A borderless text button for secondary actions ("Manually enter node URL"). */
@Composable
fun CalTextButton(
    text: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    underline: Boolean = false,
    icon: ImageVector? = null,
) {
    Row(
        modifier
            .clip(RoundedCornerShape(8.dp))
            .clickable(onClick = onClick)
            .padding(horizontal = 8.dp, vertical = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (icon != null) {
            Icon(icon, contentDescription = null, tint = Cal.textDim, modifier = Modifier.size(16.dp))
            Spacer(Modifier.width(6.dp))
        }
        Text(
            text,
            color = Cal.textDim,
            fontSize = 13.sp,
            fontWeight = FontWeight.Medium,
            textDecoration = if (underline) TextDecoration.Underline else null,
        )
    }
}

/** A grouped list section: an eyebrow over a card of divided rows. */
@Composable
fun CalSection(
    title: String,
    modifier: Modifier = Modifier,
    content: @Composable ColumnScope.() -> Unit,
) {
    Column(modifier.fillMaxWidth(), verticalArrangement = Arrangement.spacedBy(8.dp)) {
        SectionLabel(title, Modifier.padding(start = 4.dp))
        Column(
            Modifier
                .fillMaxWidth()
                .clip(RoundedCornerShape(Cal.radiusCard))
                .background(Cal.surface)
                .border(1.dp, Cal.border, RoundedCornerShape(Cal.radiusCard)),
            content = content,
        )
    }
}

/** One row in a [CalSection]: icon tile, title, optional subtitle, chevron. */
@Composable
@Suppress("LongParameterList")
fun CalRow(
    icon: ImageVector,
    title: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    subtitle: String? = null,
    accent: Boolean = false,
    chevron: Boolean = true,
) {
    Row(
        modifier
            .fillMaxWidth()
            .clickable(onClick = onClick)
            .heightIn(min = 60.dp)
            .padding(horizontal = 14.dp, vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        IconTile(icon, accent = accent, size = 36.dp)
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f)) {
            Text(title, color = Cal.text, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
            if (subtitle != null) {
                Text(subtitle, color = Cal.textFaint, fontSize = 13.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
        }
        if (chevron) Icon(Icons.Outlined.ChevronRight, contentDescription = null, tint = Cal.textFaint, modifier = Modifier.size(18.dp))
    }
}

/** A hairline between rows of a [CalSection], inset past the icon. */
@Composable
fun RowDivider() {
    Hairline(Modifier.padding(start = 62.dp))
}

/** "Live" / "Limited" pill with a dot. */
@Composable
fun LivePill(live: Boolean) {
    Row(
        Modifier
            .clip(RoundedCornerShape(999.dp))
            .background(if (live) Cal.accentSoft else Cal.warningSoft)
            .padding(horizontal = 10.dp, vertical = 5.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.size(8.dp).clip(CircleShape).background(if (live) Cal.lime else Cal.warning))
        Spacer(Modifier.width(6.dp))
        Text(
            if (live) "Live" else "Limited",
            color = if (live) Cal.accentInk else Cal.warning,
            fontSize = 12.sp,
            fontWeight = FontWeight.SemiBold,
        )
    }
}

/** Calimero avatar tones, chosen by a stable (djb2) hash of the name, the same as the Swift sample. */
object AvatarTone {
    private val tones =
        listOf(
            Color(0xFFF0FFD6) to Color(0xFF4A7300),
            Color(0xFFE6EEFB) to Color(0xFF1D4F9F),
            Color(0xFFFBE9E4) to Color(0xFF9A3412),
            Color(0xFFEFE8FB) to Color(0xFF5B3AA8),
            Color(0xFFFDF3DC) to Color(0xFF8A5300),
            Color(0xFFE2F4F1) to Color(0xFF116A5C),
        )

    fun pick(name: String): Pair<Color, Color> {
        var hash = DJB2_SEED
        for (byte in name.encodeToByteArray()) hash = (hash shl DJB2_SHIFT) + hash + (byte.toInt() and BYTE_MASK)
        return tones[Math.floorMod(hash, tones.size)]
    }

    private const val DJB2_SEED = 5381
    private const val DJB2_SHIFT = 5
    private const val BYTE_MASK = 0xFF
}

/** A coloured initials avatar. */
@Composable
fun ChatAvatar(
    name: String,
    size: Dp = 32.dp,
) {
    val (bg, fg) = AvatarTone.pick(name)
    val parts = name.split(" ").filter { it.isNotEmpty() }
    val initials =
        parts
            .take(2)
            .joinToString("") { it.take(1) }
            .ifEmpty { name.take(1) }
            .uppercase()
    Box(Modifier.size(size).clip(CircleShape).background(bg), contentAlignment = Alignment.Center) {
        Text(initials, color = fg, fontSize = (size.value * 0.4f).sp, fontWeight = FontWeight.Bold)
    }
}

/** A rounded initials tile for a space. */
@Composable
fun SpaceAvatar(
    name: String,
    size: Dp = 42.dp,
) {
    val (bg, fg) = AvatarTone.pick(name)
    Box(Modifier.size(size).clip(RoundedCornerShape(size * 0.3f)).background(bg), contentAlignment = Alignment.Center) {
        Text(name.take(1).uppercase(), color = fg, fontSize = (size.value * 0.42f).sp, fontWeight = FontWeight.Bold)
    }
}

/** A dashed rounded border, for empty states. */
fun Modifier.dashedBorder(
    color: Color,
    radius: Dp,
): Modifier =
    drawBehind {
        drawRoundRect(
            color = color,
            cornerRadius = CornerRadius(radius.toPx()),
            style = Stroke(width = 1.dp.toPx(), pathEffect = PathEffect.dashPathEffect(floatArrayOf(5.dp.toPx(), 4.dp.toPx()))),
        )
    }
