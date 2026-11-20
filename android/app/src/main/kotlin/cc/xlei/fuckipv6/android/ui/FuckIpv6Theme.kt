package cc.xlei.fuckipv6.android.ui

import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color

/**
 * The shipped product look is a flat paper surface, not a dark aurora: cream
 * ground, ink type, ink accent, no depth effects.
 *
 * Every value here mirrors `crates/common/src/design_tokens.rs`, which is the
 * single palette GPUI, WebUI and this surface must share — so the Android app
 * stops being the one screen with a navy background and a sky-blue accent.
 * Both `DARK_THEME` and `LIGHT_THEME` slots in the token file carry the same
 * paper palette, so the Android theme intentionally ignores the system
 * light/dark setting instead of inventing a second look.
 */
private val PaperScheme = lightColorScheme(
    primary = Color(0xFF1F1C17),
    onPrimary = Color(0xFFF7F3EA),
    primaryContainer = Color(0xFFE9E2D3),
    onPrimaryContainer = Color(0xFF1F1C17),
    secondary = Color(0xFF4A443A),
    onSecondary = Color(0xFFF7F3EA),
    secondaryContainer = Color(0xFFE9E2D3),
    onSecondaryContainer = Color(0xFF1F1C17),
    tertiary = Color(0xFF8A6A1F),
    onTertiary = Color(0xFFF7F3EA),
    background = Color(0xFFF2EDE2),
    onBackground = Color(0xFF1F1C17),
    surface = Color(0xFFF7F3EA),
    onSurface = Color(0xFF1F1C17),
    surfaceDim = Color(0xFFE9E2D3),
    surfaceBright = Color(0xFFFBF8F2),
    surfaceContainerLowest = Color(0xFFF7F3EA),
    surfaceContainerLow = Color(0xFFF7F3EA),
    surfaceContainer = Color(0xFFF6F1E6),
    surfaceContainerHigh = Color(0xFFF7F3EA),
    // Material cards land on the highest container: on a paper surface the
    // panel must read as raised paper, not as a recessed well.
    surfaceContainerHighest = Color(0xFFFBF8F2),
    inverseSurface = Color(0xFF1F1C17),
    inverseOnSurface = Color(0xFFF7F3EA),
    inversePrimary = Color(0xFFF2EDE2),
    surfaceVariant = Color(0xFFE9E2D3),
    onSurfaceVariant = Color(0xFF6F6555),
    surfaceTint = Color(0xFF1F1C17),
    outline = Color(0x291F1C17),
    outlineVariant = Color(0x141F1C17),
    error = Color(0xFF8E2F22),
    onError = Color(0xFFF7F3EA),
    errorContainer = Color(0xFFF2E0DC),
    onErrorContainer = Color(0xFF6E241A),
    scrim = Color(0x661F1C17),
)

@Composable
fun FuckIpv6Theme(content: @Composable () -> Unit) {
    MaterialTheme(colorScheme = PaperScheme, content = content)
}