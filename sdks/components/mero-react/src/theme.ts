/**
 * Theme tokens for mero-react components.
 *
 * Defaults match the green palette used across Calimero apps (admin-dashboard,
 * tauri-app, app-registry). Pass a partial `MeroTheme` to `ConnectButton` or
 * `LoginModal` to override any token.
 */

import type { CSSProperties } from 'react';

export interface MeroTheme {
  /** Primary accent / CTA color */
  primary?: string;
  /** Primary accent on hover */
  primaryHover?: string;
  /** Text color rendered on top of `primary` (e.g. inside the Connect button when connected) */
  primaryText?: string;
  /** Main surface color (modal background, button background) */
  background?: string;
  /** Secondary surface color (input fields, info chips) */
  backgroundSecondary?: string;
  /** Tertiary surface color (subtle elevation) */
  backgroundTertiary?: string;
  /** Border color */
  border?: string;
  /** Primary text color */
  text?: string;
  /** Muted / secondary text color */
  textSecondary?: string;
  /** Error / danger color */
  error?: string;
  /** Modal backdrop color */
  overlay?: string;
  /** Border radius (any CSS length) */
  radius?: string;
}

export type ResolvedMeroTheme = Required<MeroTheme>;

export const defaultMeroTheme: Readonly<ResolvedMeroTheme> = Object.freeze({
  primary: '#a5ff11',
  primaryHover: '#8ed40d',
  primaryText: '#0d1117',
  background: '#0d1117',
  backgroundSecondary: '#161b22',
  backgroundTertiary: '#1c2128',
  border: '#30363d',
  text: '#e6edf3',
  textSecondary: '#8b949e',
  error: '#ff6b6b',
  overlay: 'rgba(0, 0, 0, 0.75)',
  radius: '8px',
});

/**
 * Merge a partial theme with the defaults.
 *
 * Filtering rules — values that fall back to the default:
 *   - `undefined` (the prop key wasn't set, or was set to `undefined`)
 *   - empty string `''`
 *   - whitespace-only strings (e.g. `'   '`)
 *
 * These are dropped because they're invalid CSS and would produce
 * invisible elements (transparent backgrounds, missing borders, etc.).
 * If you genuinely want the browser default for a token, omit the key
 * from the theme prop rather than passing an empty string.
 *
 * Any other non-empty string is forwarded as-is — CSS syntax itself
 * is NOT validated.
 */
export function resolveMeroTheme(theme?: MeroTheme): ResolvedMeroTheme {
  if (!theme) return { ...defaultMeroTheme };
  const filtered = Object.fromEntries(
    Object.entries(theme).filter(
      ([, v]) => v !== undefined && typeof v === 'string' && v.trim() !== '',
    ),
  ) as Partial<MeroTheme>;
  return { ...defaultMeroTheme, ...filtered };
}

/**
 * Single source of truth mapping each `MeroTheme` token name to the CSS
 * custom-property name it surfaces as. Used by `themeToCssVars` to emit the
 * inline-style map and by component code (e.g. `LoginModal`) to read the
 * variable in inline styles via `var(--mero-*, fallback)`.
 *
 * Adding a new token? Add it here, in `MeroTheme`, and in `defaultMeroTheme`,
 * and the rest of the system picks it up automatically.
 */
export const MERO_CSS_VARS: Record<keyof MeroTheme, string> = {
  background: '--mero-bg',
  backgroundSecondary: '--mero-bg-secondary',
  backgroundTertiary: '--mero-bg-tertiary',
  text: '--mero-text',
  textSecondary: '--mero-text-secondary',
  primary: '--mero-accent',
  primaryHover: '--mero-accent-hover',
  primaryText: '--mero-on-primary',
  border: '--mero-border',
  error: '--mero-error',
  overlay: '--mero-overlay',
  radius: '--mero-radius',
};

/**
 * Build a `style` object that exposes a resolved theme as CSS variables, so
 * descendant CSS rules (e.g. those in `styles.css`) pick up the overrides.
 */
export function themeToCssVars(theme: ResolvedMeroTheme): CSSProperties {
  const vars: Record<string, string> = {};
  for (const key of Object.keys(MERO_CSS_VARS) as Array<keyof MeroTheme>) {
    vars[MERO_CSS_VARS[key]] = theme[key];
  }
  return vars as CSSProperties;
}

/**
 * Returns a `var(--mero-*, fallback)` reference for a theme token, suitable
 * for inline styles in portal-rendered components where descendant CSS
 * variables also need to support global `:root` overrides.
 */
export function cssVar(theme: ResolvedMeroTheme, key: keyof MeroTheme): string {
  return `var(${MERO_CSS_VARS[key]}, ${theme[key]})`;
}
