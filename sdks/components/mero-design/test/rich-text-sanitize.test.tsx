// @vitest-environment jsdom
import { describe, expect, it } from 'vitest';
import * as ui from '../src';

// Rich text a peer sent is attacker-controlled. Apps render it outside the
// editor (message lists, previews), so mero-design has to hand them HTML cut down
// to what the editor itself can produce.
describe('sanitizeRichTextHtml', () => {
  const sanitize = (html: string): string =>
    (ui as unknown as { sanitizeRichTextHtml: (h: string) => string }).sanitizeRichTextHtml(html);

  it('is exported', () => {
    expect(typeof (ui as Record<string, unknown>).sanitizeRichTextHtml).toBe('function');
  });

  it('drops script, event handlers and unknown elements', () => {
    const out = sanitize(
      '<p>hi<img src=x onerror="alert(1)"><script>alert(2)</script>' +
        '<iframe src="https://evil.example"></iframe><b onclick="alert(3)">b</b></p>' +
        '<svg><a xlink:href="javascript:alert(4)"><text>t</text></a></svg>',
    );
    expect(out).toContain('hi');
    expect(out).toContain('<strong>b</strong>');
    expect(out).not.toMatch(/<img|<script|<iframe|<svg|onerror|onclick|alert/i);
  });

  it('drops javascript:, data: and vbscript: links but keeps https links', () => {
    const out = sanitize(
      '<p><a href="javascript:alert(1)">a</a><a href=" JaVaScRiPt:alert(1)">b</a>' +
        '<a href="data:text/html,<script>alert(1)</script>">c</a>' +
        '<a href="vbscript:msgbox(1)">d</a><a href="https://calimero.network">ok</a></p>',
    );
    expect(out).not.toMatch(/javascript:|data:|vbscript:/i);
    expect(out).toContain('href="https://calimero.network"');
    expect(out).toContain('rel="noopener noreferrer nofollow"');
  });

  it('drops CSS other than a valid colour and alignment', () => {
    const out = sanitize(
      '<p style="text-align: center; background: url(https://evil.example/t)">' +
        '<span style="color: red; background-image: url(https://evil.example/t)">x</span></p>',
    );
    expect(out).not.toContain('evil.example');
    expect(out).toContain('text-align: center');
    expect(out).toContain('color: red');
  });

  it('returns an empty string for empty input', () => {
    expect(sanitize('')).toBe('');
  });
});
