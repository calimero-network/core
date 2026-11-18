import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it } from 'vitest';
import { Input, Textarea } from '../src';

// A recovery phrase or key typed into a text field is sent to the browser's
// cloud spellchecker unless spellcheck is off, and kept in autofill history
// unless autocomplete is off. Apps have to be able to turn both off.
describe('secret-safe text fields', () => {
  it('Input passes spellCheck through', () => {
    const html = renderToStaticMarkup(
      <Input type="text" autoComplete="off" spellCheck={false} />,
    );
    expect(html).toContain('spellCheck="false"');
    expect(html).toContain('autoComplete="off"');
  });

  it('Textarea passes spellCheck and autoComplete through', () => {
    const html = renderToStaticMarkup(
      <Textarea autoComplete="off" spellCheck={false} />,
    );
    expect(html).toContain('spellCheck="false"');
    expect(html).toContain('autoComplete="off"');
  });
});
