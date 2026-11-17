import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { CommandPalette, Icon } from '../src';

// Names that come from shared data (a channel's category, an icon picked by
// another member) must not reach Object.prototype and crash the render.
const keys = ['__proto__', 'constructor', 'toString', 'hasOwnProperty'];

describe('names that are Object.prototype keys', () => {
  it.each(keys)('CommandPalette renders a "%s" category', (category) => {
    expect(() =>
      renderToStaticMarkup(
        <CommandPalette items={[{ id: '1', title: 't', category, action() {} }]} />,
      ),
    ).not.toThrow();
  });

  it.each(keys)('Icon renders the fallback for "%s"', (name) => {
    const html = renderToStaticMarkup(<Icon name={name} />);
    expect(html).toContain('<path');
  });
});
