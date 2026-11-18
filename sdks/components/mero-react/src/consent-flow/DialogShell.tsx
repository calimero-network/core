import '../create-workspace/workspaceReview.css';
import '../create-workspace/createWorkspace.css';
import { useMemo, useSyncExternalStore, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import type { CalimeroClient, PasskeyProvider } from '@calimero-network/mero-js';
import { MeroProvider } from '../context';

const subscribeNothing = () => () => undefined;
/** False on the server and during hydration, true after: portals need `document`. */
export const useMounted = () => useSyncExternalStore(subscribeNothing, () => true, () => false);

const CloseIcon = () => (
  <svg viewBox="0 0 24 24" width="16" height="16" fill="none" aria-hidden="true">
    <path d="m6 6 12 12M18 6 6 18" stroke="currentColor" strokeWidth="2" strokeLinecap="round" />
  </svg>
);

/** The shared dialog frame: a portal on `document.body` above host overlays, with a Close button and Escape. */
export function DialogShell({ label, locked = false, onClose, children }: { label: string; locked?: boolean; onClose: () => void; children: ReactNode }) {
  const mounted = useMounted();
  if (!mounted) return null;
  return createPortal(
    <div className="mero-cw mero-cw-overlay" onKeyDown={(e) => e.key === 'Escape' && !locked && onClose()}>
      <div className="mero-cw-card" role="dialog" aria-modal="true" aria-label={label}>
        <button type="button" className="mero-cw-x" aria-label="Close" onClick={onClose}>
          <CloseIcon />
        </button>
        <div className="mero-cw-body">{children}</div>
      </div>
    </div>,
    document.body,
  );
}

/** The app's own client in a `MeroProvider`; without one the ambient provider is used. */
export function WithClient({ client, passkey, children }: { client?: CalimeroClient; passkey?: PasskeyProvider; children: ReactNode }) {
  const sessionStore = useMemo(() => ({ load: () => client?.session ?? null, save: () => undefined, clear: () => undefined }), [client]);
  return client ? (
    <MeroProvider client={client} sessionStore={sessionStore} passkey={passkey}>
      {children}
    </MeroProvider>
  ) : (
    <>{children}</>
  );
}

/** "Approve with your passkey": the pop-up's waiting card, or the full-page trip's code card. */
export function ApprovalCard({
  redirecting,
  code,
  onCancel,
  onProceed,
  onReopen,
}: {
  redirecting: boolean;
  code: string | null;
  onCancel: () => void;
  onProceed: () => void;
  onReopen: () => void;
}) {
  return (
    <div className="mero-cw-approve">
      <h2 className="mero-cw-title">Approve with your passkey</h2>
      <p className="mero-cw-sub">
        {redirecting
          ? 'Your node will open its own page to ask for your passkey. Check that it shows this code.'
          : 'A window from your node opened. Check it shows this code, then approve there.'}
      </p>
      <div className="mero-cw-code" aria-live="polite">
        {code ?? '...'}
      </div>
      {redirecting ? (
        <>
          <button type="button" className="mero-cw-btn mero-cw-btn--primary mero-cw-btn--full" onClick={onProceed}>
            Continue to your node
          </button>
          <div className="mero-cw-row">
            <button type="button" className="mero-cw-btn" onClick={onCancel}>
              Cancel
            </button>
          </div>
        </>
      ) : (
        <>
          <div className="mero-cw-wait" role="status">
            <span className="mero-cw-dot" aria-hidden="true" />
            Waiting for your approval...
          </div>
          <div className="mero-cw-row">
            <button type="button" className="mero-cw-btn" onClick={onCancel}>
              Cancel
            </button>
            <button type="button" className="mero-cw-link" onClick={onReopen}>
              Open the window again
            </button>
          </div>
        </>
      )}
    </div>
  );
}
