// @vitest-environment node
import { renderToString } from 'react-dom/server';
import { describe, expect, it, vi } from 'vitest';
import type { CalimeroClient } from '@calimero-network/mero-js';
import { NodeShield } from '../node-shield';

const client = {
  nodeUrl: 'https://node-3.relay.example.net',
  verification: () => undefined,
  verify: () => new Promise(() => {}),
} as unknown as CalimeroClient;

describe('NodeShield on the server', () => {
  it('renders the checking chip without touching window, and without warnings', () => {
    const errors = vi.spyOn(console, 'error').mockImplementation(() => {});
    expect(typeof window).toBe('undefined');
    const html = renderToString(<NodeShield client={client} compactBelow={700} />);
    expect(html).toContain('Checking node');
    expect(html).not.toContain('mero-node-shield--compact');
    expect(errors).not.toHaveBeenCalled();
    errors.mockRestore();
  });
});
