@file:Suppress("MatchingDeclarationName", "TooManyFunctions")

package com.calimero.mero.sample.ui

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.ArrowBack
import androidx.compose.material.icons.outlined.ChevronRight
import androidx.compose.material.icons.outlined.ContentCopy
import androidx.compose.material.icons.outlined.ErrorOutline
import androidx.compose.material.icons.outlined.Info
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.OutlinedTextFieldDefaults
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

/**
 * Calimero light design tokens (mero-vote `index.css`, the canonical light redesign).
 * System font throughout: Power Grotesk is not licensed for app embedding.
 */
object Cal {
    val bg = Color(0xFFF6F6F3)
    val bgSubtle = Color(0xFFEFEFEB)
    val surface = Color(0xFFFFFFFF)
    val surfaceHover = Color(0xFFF3F3F0)
    val surfaceSunken = Color(0xFFF1F1EE)
    val border = Color(0xFFE5E5E0)
    val borderStrong = Color(0xFFD4D4CE)
    val text = Color(0xFF131215)
    val textDim = Color(0xFF4A4A4F)
    val textFaint = Color(0xFF6B6B70)
    val lime = Color(0xFFA5FF11)
    val limePressed = Color(0xFFB4FF3A)
    val accentSoft = Color(0xFFF0FFD6)
    val accentInk = Color(0xFF4A7300)
    val onLime = Color(0xFF131215)
    val limeEdge = Color(0x0F000000)
    val error = Color(0xFFC62828)
    val errorSoft = Color(0xFFFDECEC)
    val warning = Color(0xFF9A5B00)
    val warningSoft = Color(0xFFFFF4E0)
    val info = Color(0xFF1D5FBF)
    val infoSoft = Color(0xFFEAF1FC)
    val success = Color(0xFF2F7A00)
    val successSoft = Color(0xFFEEF8E4)

    val radiusControl = 12.dp
    val radiusCard = 18.dp
    val cardPad = 20.dp
}

/** App-wide horizontal screen gutter. */
val screenPad = 16.dp

/** Light Material theme mapped onto the Calimero tokens. */
@Composable
fun MeroExplorerTheme(content: @Composable () -> Unit) {
    val scheme =
        lightColorScheme(
            primary = Cal.text,
            onPrimary = Cal.surface,
            secondary = Cal.accentInk,
            background = Cal.bg,
            onBackground = Cal.text,
            surface = Cal.surface,
            onSurface = Cal.text,
            surfaceVariant = Cal.surfaceSunken,
            onSurfaceVariant = Cal.textDim,
            outline = Cal.borderStrong,
            outlineVariant = Cal.border,
            error = Cal.error,
        )
    MaterialTheme(colorScheme = scheme, content = content)
}

/** Lime primary button: lime fill, ink text, 1px dark hairline. */
@Composable
fun CalPrimaryButton(
    text: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    enabled: Boolean = true,
    icon: ImageVector? = null,
) {
    Button(
        onClick = onClick,
        enabled = enabled,
        modifier = modifier.heightIn(min = 52.dp),
        shape = RoundedCornerShape(Cal.radiusControl),
        border = BorderStroke(1.dp, Cal.limeEdge),
        contentPadding = PaddingValues(horizontal = 16.dp),
        colors =
            ButtonDefaults.buttonColors(
                containerColor = Cal.lime,
                contentColor = Cal.onLime,
                disabledContainerColor = Cal.lime.copy(alpha = 0.5f),
                disabledContentColor = Cal.onLime.copy(alpha = 0.5f),
            ),
    ) { ButtonContent(text, icon) }
}

/** White secondary button with a strong hairline. */
@Composable
fun CalSecondaryButton(
    text: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    enabled: Boolean = true,
    icon: ImageVector? = null,
    danger: Boolean = false,
) {
    OutlinedButton(
        onClick = onClick,
        enabled = enabled,
        modifier = modifier.heightIn(min = 48.dp),
        shape = RoundedCornerShape(Cal.radiusControl),
        border = BorderStroke(1.dp, Cal.borderStrong),
        contentPadding = PaddingValues(horizontal = 16.dp),
        colors =
            ButtonDefaults.outlinedButtonColors(
                containerColor = Cal.surface,
                contentColor = if (danger) Cal.error else Cal.text,
            ),
    ) { ButtonContent(text, icon) }
}

/** Text-only button in textDim. */
@Composable
fun CalPlainButton(
    text: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    icon: ImageVector? = null,
) {
    TextButton(
        onClick = onClick,
        modifier = modifier,
        shape = RoundedCornerShape(Cal.radiusControl),
        colors = ButtonDefaults.textButtonColors(contentColor = Cal.textDim),
    ) { ButtonContent(text, icon) }
}

@Composable
private fun RowScope.ButtonContent(
    text: String,
    icon: ImageVector?,
) {
    if (icon != null) {
        Icon(icon, contentDescription = null, modifier = Modifier.size(18.dp))
        Spacer(Modifier.width(8.dp))
    }
    Text(text, fontWeight = FontWeight.SemiBold, fontSize = 15.sp)
}

/** A white card: 1px hairline, radius 14, padding 20. */
@Composable
fun CalCard(
    modifier: Modifier = Modifier,
    padding: Dp = Cal.cardPad,
    content: @Composable ColumnScope.() -> Unit,
) {
    Column(
        modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(Cal.radiusCard))
            .background(Cal.surface)
            .border(1.dp, Cal.border, RoundedCornerShape(Cal.radiusCard))
            .padding(padding),
        content = content,
    )
}

/** A card head: 34px icon tile + title + optional meta. */
@Composable
fun CardHead(
    icon: ImageVector,
    title: String,
    meta: String? = null,
    accent: Boolean = false,
    trailing: @Composable (() -> Unit)? = null,
) {
    Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.fillMaxWidth()) {
        IconTile(icon, accent = accent)
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f)) {
            Text(title, color = Cal.text, fontSize = 15.sp, fontWeight = FontWeight.SemiBold)
            if (meta != null) Text(meta, color = Cal.textFaint, fontSize = 12.5.sp)
        }
        trailing?.invoke()
    }
}

/** A 34px rounded icon tile (bgSubtle / textDim, or accentSoft / accentInk). */
@Composable
fun IconTile(
    icon: ImageVector,
    accent: Boolean = false,
    size: Dp = 34.dp,
) {
    Box(
        Modifier
            .size(size)
            .clip(RoundedCornerShape(size * 0.3f))
            .background(if (accent) Cal.accentSoft else Cal.bgSubtle),
        contentAlignment = Alignment.Center,
    ) {
        Icon(icon, contentDescription = null, tint = if (accent) Cal.accentInk else Cal.textDim, modifier = Modifier.size(size * 0.52f))
    }
}

/** The Calimero mark: a lime square with a dark hairline and an ink cut-out. */
@Composable
fun CalMark(size: Dp = 28.dp) {
    Box(
        Modifier
            .size(size)
            .clip(RoundedCornerShape(size * 0.28f))
            .background(Cal.lime)
            .border(1.dp, Cal.limeEdge, RoundedCornerShape(size * 0.28f)),
        contentAlignment = Alignment.Center,
    ) {
        Box(
            Modifier
                .size(size * 0.34f)
                .clip(RoundedCornerShape(size * 0.1f))
                .background(Cal.onLime),
        )
    }
}

/** Brand mark + app name. */
@Composable
fun CalLogo(
    modifier: Modifier = Modifier,
    name: String = "Calimero",
) {
    Row(modifier = modifier, verticalAlignment = Alignment.CenterVertically) {
        CalMark(28.dp)
        Spacer(Modifier.width(10.dp))
        Text(name, color = Cal.text, fontSize = 17.sp, fontWeight = FontWeight.Bold, letterSpacing = (-0.17).sp)
    }
}

/** A white top bar with a bottom hairline: optional back button, title, subtitle, trailing actions. */
@Composable
fun TopBar(
    title: String,
    modifier: Modifier = Modifier,
    subtitle: String? = null,
    onBack: (() -> Unit)? = null,
    actions: @Composable RowScope.() -> Unit = {},
) {
    Column(modifier.fillMaxWidth().background(Cal.surface).statusBarsPadding()) {
        Row(
            Modifier.fillMaxWidth().heightIn(min = 56.dp).padding(horizontal = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            if (onBack != null) {
                IconButton(onClick = onBack) {
                    Icon(Icons.AutoMirrored.Outlined.ArrowBack, contentDescription = "Back", tint = Cal.text)
                }
            } else {
                Spacer(Modifier.width(8.dp))
                CalMark(28.dp)
                Spacer(Modifier.width(10.dp))
            }
            Column(Modifier.weight(1f)) {
                Text(title, color = Cal.text, fontSize = 17.sp, fontWeight = FontWeight.Bold, maxLines = 1, overflow = TextOverflow.Ellipsis)
                if (subtitle != null) {
                    Text(subtitle, color = Cal.textFaint, fontSize = 13.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
                }
            }
            actions()
        }
        Box(Modifier.fillMaxWidth().height(1.dp).background(Cal.border))
    }
}

/**
 * Single-line (or multi-line) text input: white, strong hairline, ink focus. [modifier] goes on
 * the field itself (so a test tag lands on the editable node); [password] masks the text.
 */
@Composable
@Suppress("LongParameterList")
fun CalTextField(
    value: String,
    onValueChange: (String) -> Unit,
    label: String,
    modifier: Modifier = Modifier,
    singleLine: Boolean = true,
    enabled: Boolean = true,
    placeholder: String? = null,
    mono: Boolean = false,
    password: Boolean = false,
    keyboardOptions: KeyboardOptions = KeyboardOptions.Default,
    keyboardActions: KeyboardActions = KeyboardActions.Default,
) {
    Column(Modifier.fillMaxWidth()) {
        Text(label, color = Cal.text, fontSize = 13.sp, fontWeight = FontWeight.Medium)
        Spacer(Modifier.height(6.dp))
        OutlinedTextField(
            value = value,
            onValueChange = onValueChange,
            singleLine = singleLine,
            enabled = enabled,
            visualTransformation = if (password) PasswordVisualTransformation() else VisualTransformation.None,
            keyboardOptions = keyboardOptions,
            keyboardActions = keyboardActions,
            placeholder = placeholder?.let { { Text(it, color = Cal.textFaint, fontSize = 14.sp) } },
            textStyle =
                MaterialTheme.typography.bodyMedium.copy(
                    fontFamily = if (mono) FontFamily.Monospace else FontFamily.Default,
                    color = Cal.text,
                ),
            shape = RoundedCornerShape(Cal.radiusControl),
            modifier = modifier.fillMaxWidth(),
            colors =
                OutlinedTextFieldDefaults.colors(
                    focusedContainerColor = Cal.surface,
                    unfocusedContainerColor = Cal.surface,
                    disabledContainerColor = Cal.surfaceSunken,
                    focusedBorderColor = Cal.text,
                    unfocusedBorderColor = Cal.borderStrong,
                    cursorColor = Cal.text,
                ),
        )
    }
}

/** Callout kinds. */
enum class CalloutKind { INFO, SUCCESS, WARNING, DANGER }

/** A soft-tinted callout with a leading icon, an optional bold title, and body text. */
@Composable
fun Callout(
    body: String,
    modifier: Modifier = Modifier,
    title: String? = null,
    kind: CalloutKind = CalloutKind.INFO,
    content: @Composable ColumnScope.() -> Unit = {},
) {
    val (bg, fg) =
        when (kind) {
            CalloutKind.INFO -> Cal.infoSoft to Cal.info
            CalloutKind.SUCCESS -> Cal.successSoft to Cal.success
            CalloutKind.WARNING -> Cal.warningSoft to Cal.warning
            CalloutKind.DANGER -> Cal.errorSoft to Cal.error
        }
    Row(
        modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(10.dp))
            .background(bg)
            .border(1.dp, fg.copy(alpha = 0.18f), RoundedCornerShape(10.dp))
            .padding(horizontal = 14.dp, vertical = 12.dp),
    ) {
        Icon(
            if (kind == CalloutKind.DANGER) Icons.Outlined.ErrorOutline else Icons.Outlined.Info,
            contentDescription = null,
            tint = fg,
            modifier = Modifier.size(18.dp),
        )
        Spacer(Modifier.width(10.dp))
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(4.dp)) {
            if (title != null) Text(title, color = Cal.text, fontSize = 13.5.sp, fontWeight = FontWeight.SemiBold)
            Text(body, color = Cal.textDim, fontSize = 13.5.sp, lineHeight = 19.sp)
            content()
        }
    }
}

/** A status pill with a dot: live (lime) or waiting (warning). */
@Composable
fun StatusPill(
    text: String,
    live: Boolean,
    modifier: Modifier = Modifier,
) {
    Row(
        modifier
            .clip(RoundedCornerShape(999.dp))
            .background(Cal.bgSubtle)
            .padding(horizontal = 10.dp, vertical = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.size(8.dp).clip(CircleShape).background(if (live) Cal.lime else Cal.warning))
        Spacer(Modifier.width(6.dp))
        Text(text, color = Cal.textDim, fontSize = 12.sp, fontWeight = FontWeight.Medium)
    }
}

/** UPPERCASE 12px eyebrow label. */
@Composable
fun SectionLabel(
    text: String,
    modifier: Modifier = Modifier,
) {
    Text(
        text.uppercase(),
        color = Cal.textFaint,
        fontSize = 12.sp,
        fontWeight = FontWeight.Medium,
        letterSpacing = 0.5.sp,
        modifier = modifier,
    )
}

/** A label + a sunken mono id field with a copy button. */
@Composable
fun IdField(
    label: String,
    value: String,
    modifier: Modifier = Modifier,
) {
    val clipboard = LocalClipboardManager.current
    Column(modifier.fillMaxWidth()) {
        Text(label, color = Cal.textFaint, fontSize = 12.sp)
        Spacer(Modifier.height(4.dp))
        Row(
            Modifier
                .fillMaxWidth()
                .clip(RoundedCornerShape(6.dp))
                .background(Cal.surfaceSunken)
                .border(1.dp, Cal.border, RoundedCornerShape(6.dp))
                .padding(start = 10.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text(
                value,
                color = Cal.textDim,
                fontSize = 12.5.sp,
                fontFamily = FontFamily.Monospace,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f),
            )
            IconButton(onClick = { clipboard.setText(AnnotatedString(value)) }, modifier = Modifier.size(32.dp)) {
                Icon(Icons.Outlined.ContentCopy, contentDescription = "Copy $label", tint = Cal.textFaint, modifier = Modifier.size(16.dp))
            }
        }
    }
}

/** "Show technical details" disclosure: a hairline, a rotating chevron, and the contents when open. */
@Composable
fun TechDetails(
    modifier: Modifier = Modifier,
    label: String = "Show technical details",
    content: @Composable ColumnScope.() -> Unit,
) {
    var open by remember { mutableStateOf(false) }
    Column(modifier.fillMaxWidth()) {
        Spacer(Modifier.height(16.dp))
        Box(Modifier.fillMaxWidth().height(1.dp).background(Cal.border))
        Row(
            Modifier
                .fillMaxWidth()
                .clickable { open = !open }
                .padding(vertical = 12.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Icon(
                Icons.Outlined.ChevronRight,
                contentDescription = null,
                tint = Cal.textFaint,
                modifier = Modifier.size(16.dp).rotate(if (open) 90f else 0f),
            )
            Spacer(Modifier.width(6.dp))
            Text(if (open) "Hide technical details" else label, color = Cal.textFaint, fontSize = 13.sp, fontWeight = FontWeight.Medium)
        }
        AnimatedVisibility(open) {
            Column(verticalArrangement = Arrangement.spacedBy(10.dp), content = content)
        }
    }
}

/** Dashed-border empty state: icon tile, title, body, optional action. */
@Composable
fun EmptyState(
    icon: ImageVector,
    title: String,
    body: String,
    modifier: Modifier = Modifier,
    action: @Composable (() -> Unit)? = null,
) {
    Column(
        modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(Cal.radiusCard))
            .background(Cal.surface.copy(alpha = 0.6f))
            .dashedBorder(Cal.borderStrong, Cal.radiusCard)
            .padding(horizontal = 20.dp, vertical = 32.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        IconTile(icon, size = 40.dp)
        Text(title, color = Cal.text, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, textAlign = TextAlign.Center)
        Text(body, color = Cal.textDim, fontSize = 14.sp, lineHeight = 20.sp, textAlign = TextAlign.Center)
        action?.invoke()
    }
}

/** A tappable list row: icon tile, title + meta, trailing chevron. */
@Composable
fun ListRow(
    icon: ImageVector,
    title: String,
    meta: String?,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    accent: Boolean = false,
) {
    Row(
        modifier
            .fillMaxWidth()
            .clickable(onClick = onClick)
            .heightIn(min = 64.dp)
            .padding(horizontal = 20.dp, vertical = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        IconTile(icon, accent = accent)
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f)) {
            Text(title, color = Cal.text, fontSize = 15.sp, fontWeight = FontWeight.SemiBold, maxLines = 1, overflow = TextOverflow.Ellipsis)
            if (meta != null) Text(meta, color = Cal.textFaint, fontSize = 12.5.sp, maxLines = 2, overflow = TextOverflow.Ellipsis)
        }
        Icon(Icons.Outlined.ChevronRight, contentDescription = null, tint = Cal.textFaint, modifier = Modifier.size(16.dp))
    }
}

/** A 1px hairline divider. */
@Composable
fun Hairline(modifier: Modifier = Modifier) {
    Box(modifier.fillMaxWidth().height(1.dp).background(Cal.border))
}

/** Shorten a 64-hex id for display: `abcd1234…9f0e`. */
fun shortId(id: String?): String = if (id == null || id.length <= 14) id.orEmpty() else id.take(8) + "…" + id.takeLast(4)
