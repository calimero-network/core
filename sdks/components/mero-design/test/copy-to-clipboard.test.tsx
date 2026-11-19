// @vitest-environment jsdom
import { act } from 'react';
import { createRoot } from 'react-dom/client';
import { afterEach, describe, expect, it } from 'vitest';
import { useCopyToClipboard } from '../src';

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

// Without the async clipboard API the copy goes through a hidden textarea.
// The text is often a secret, so it must leave the DOM even if copying fails.
describe('useCopyToClipboard fallback', () => {
  const original = document.execCommand;
  afterEach(() => {
    document.execCommand = original;
    document.body.innerHTML = '';
  });

  it('removes the hidden textarea when copying throws', async () => {
    document.execCommand = () => {
      throw new Error('copy blocked');
    };
    let copy: ((text: string) => Promise<unknown>) | undefined;
    function Probe() {
      copy = useCopyToClipboard().copy;
      return null;
    }
    const host = document.createElement('div');
    document.body.appendChild(host);
    const root = createRoot(host);
    await act(async () => root.render(<Probe />));
    await act(async () => {
      await copy!('recovery phrase words');
    });
    expect(document.querySelectorAll('textarea')).toHaveLength(0);
    expect(document.body.innerHTML).not.toContain('recovery phrase');
    await act(async () => root.unmount());
  });
});
