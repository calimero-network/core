// @vitest-environment node
import { renderToString } from 'react-dom/server';
import { describe, expect, it, vi } from 'vitest';
import { ConsentPopupReturn, CreateWorkspaceDialog } from '../create-workspace';
import { MockMeroProvider } from '../test-utils/MockMeroProvider';

describe('create workspace on the server', () => {
  it('renders the app, and the portalled dialog only after mount, without touching window', () => {
    expect(typeof window).toBe('undefined');
    const errors = vi.spyOn(console, 'error').mockImplementation(() => {});
    const html = renderToString(
      <MockMeroProvider>
        <ConsentPopupReturn>
          <p>the app</p>
          <CreateWorkspaceDialog open release={{ package: 'p', version: '1' }} onCreated={() => {}} onClose={() => {}} />
        </ConsentPopupReturn>
      </MockMeroProvider>,
    );
    expect(html).toContain('the app');
    expect(html).not.toContain('Workspace name');
    expect(errors).not.toHaveBeenCalled();
    errors.mockRestore();
  });
});
