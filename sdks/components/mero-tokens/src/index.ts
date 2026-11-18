// Export the raw tokens data
import tokens from "../tokens.json";

// Export the tokens object
export { tokens };

// Export individual token categories
export const colors = tokens.color;
export const spacing = tokens.space;
export const radius = tokens.radius;
export const fonts = tokens.font;

// Export CSS variables as a string (for runtime use)
export const cssVariables = `
:root {
  --color-neutral-200: ${colors.neutral["200"].value};
  --color-neutral-300: ${colors.neutral["300"].value};
  --color-neutral-400: ${colors.neutral["400"].value};
  --color-neutral-600: ${colors.neutral["600"].value};
  --color-neutral-700: ${colors.neutral["700"].value};
  --color-neutral-800: ${colors.neutral["800"].value};
  --color-neutral-900: ${colors.neutral["900"].value};
  --color-brand-100: ${colors.brand["100"].value};
  --color-brand-600: ${colors.brand["600"].value};
  --color-brand-700: ${colors.brand["700"].value};
  --color-brand-800: ${colors.brand["800"].value};
  --color-brand-900: ${colors.brand["900"].value};
  --color-transparent-brand: ${colors.transparent.brand.value};
  --color-transparent-black-50: ${colors.transparent["black-50"].value};
  --color-transparent-green-20: ${colors.transparent["green-20"].value};
  --color-transparent-green-10: ${colors.transparent["green-10"].value};
  --color-background-primary: ${colors.background.primary.value};
  --color-background-secondary: ${colors.background.secondary.value};
  --color-background-tertiary: ${colors.background.tertiary.value};
  --color-background-brand: ${colors.background.brand.value};
  --color-semantic-success: ${colors.semantic.success.value};
  --color-semantic-warning: ${colors.semantic.warning.value};
  --color-semantic-error: ${colors.semantic.error.value};
  --color-semantic-info: ${colors.semantic.info.value};
  --radius-sm: ${radius.sm.value};
  --radius-md: ${radius.md.value};
  --radius-lg: ${radius.lg.value};
  --space-2: ${spacing["2"].value};
  --space-3: ${spacing["3"].value};
  --space-4: ${spacing["4"].value};
  --space-6: ${spacing["6"].value};
  --font-body: ${fonts.body.value};
  --font-heading: ${fonts.heading.value};
  --font-secondary: ${fonts.secondary.value};
}
`;
