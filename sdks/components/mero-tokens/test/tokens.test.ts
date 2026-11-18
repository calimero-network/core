import { describe, expect, it } from 'vitest';
import { colors, cssVariables, fonts, radius, spacing, tokens } from '../src';

describe('mero-tokens', () => {
  it('exports every token category', () => {
    expect(tokens.color).toBe(colors);
    expect(spacing).toBeTypeOf('object');
    expect(radius).toBeTypeOf('object');
    expect(fonts).toBeTypeOf('object');
  });

  it('renders the CSS variables from the token values', () => {
    expect(cssVariables).toContain(':root {');
    expect(cssVariables).toContain(`--color-background-brand: ${colors.background.brand.value};`);
    expect(cssVariables).not.toContain('undefined');
  });
});
