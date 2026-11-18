import * as React from 'react';
import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import * as icons from '../src';

const components = Object.entries(icons).filter(
  ([name, value]) => /^[A-Z]/.test(name) && typeof value === 'object' && value !== null,
) as [string, React.ComponentType<icons.IconProps>][];

describe('mero-icons', () => {
  it('exports icons', () => {
    expect(components.length).toBeGreaterThan(50);
  });

  it.each(components)('%s renders an svg at the requested size', (_name, Icon) => {
    const html = renderToStaticMarkup(<Icon size={16} />);
    expect(html).toMatch(/^<svg /);
    expect(html).toContain('width="16"');
  });
});
