import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import * as ui from '../src';
import { Button, Card } from '../src';

describe('mero-ui', () => {
  it('defines every export', () => {
    const missing = Object.entries(ui)
      .filter(([, value]) => value === undefined)
      .map(([name]) => name);
    expect(missing).toEqual([]);
    expect(Object.keys(ui).length).toBeGreaterThan(50);
  });

  it('renders a Button with its label', () => {
    const html = renderToStaticMarkup(<Button variant="secondary">Save</Button>);
    expect(html).toContain('<button');
    expect(html).toContain('Save');
  });

  it('renders a Card with its children', () => {
    const html = renderToStaticMarkup(<Card>content</Card>);
    expect(html).toContain('content');
  });
});
