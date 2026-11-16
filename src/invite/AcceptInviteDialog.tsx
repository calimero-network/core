import { useState } from 'react';
import type { CalimeroClient, PasskeyProvider } from '@calimero-network/mero-js';
import { ApprovalCard, DialogShell, WithClient } from '../consent-flow/DialogShell';
import { useAcceptResume, useInviteAccept } from './useInvite';

export interface AcceptInviteDialogProps {
  open: boolean;
  onClose: () => void;
  /** The invite token from the link (`readInviteToken`, or `useInviteRedemption`'s `token`). */
  token: string;
  appName?: string;
  /** Called once the person has joined, from the dialog or from a full-page approval. */
  onJoined: (joined: { org: string }) => void;
  /** Where the node's consent page sends the person back. Default: this origin's root, where the app mounts `ConsentPopupReturn`. */
  returnTo?: string;
  client?: CalimeroClient;
  passkey?: PasskeyProvider;
}

/**
 * Join a workspace with an invite: asks the node's consent page for a session
 * bound to the app, else redeems the invite directly. Mount it once and toggle
 * `open`; it also finishes a join approved on a full-page trip to the node.
 */
export function AcceptInviteDialog({ client, passkey, ...props }: AcceptInviteDialogProps) {
  return (
    <WithClient client={client} passkey={passkey}>
      <Dialog {...props} />
    </WithClient>
  );
}

function Dialog({ open, onClose, token, appName, onJoined, returnTo }: Omit<AcceptInviteDialogProps, 'client' | 'passkey'>) {
  const flow = useInviteAccept({ returnTo: returnTo ?? `${typeof window === 'undefined' ? '' : window.location.origin}/`, onJoined });
  const resume = useAcceptResume({ onJoined });
  const [dismissed, setDismissed] = useState<string | null>(null);
  const notice = resume.message && dismissed !== resume.message ? resume.message : null;
  const redirecting = flow.phase === 'redirect';
  const approving = flow.phase === 'approving' || redirecting;
  if (!open && !notice) return null;
  return (
    <DialogShell label="Join workspace" locked={approving} onClose={() => (approving ? flow.cancel() : onClose())}>
      {approving ? (
        <ApprovalCard redirecting={redirecting} code={flow.code} onCancel={flow.cancel} onProceed={flow.proceed} onReopen={flow.reopen} />
      ) : (
        <div>
          <h2 className="mero-cw-title">Join workspace</h2>
          <p className="mero-cw-sub">{`You were invited to a workspace${appName ? ` in ${appName}` : ''}. Join it with your account.`}</p>
          {[notice, flow.error].filter(Boolean).map((message) => (
            <p key={message} className="mero-cw-error" role="alert">
              {message}
              {message === notice && (
                <>
                  {' '}
                  <button type="button" className="mero-cw-link" onClick={() => setDismissed(message)}>
                    Dismiss
                  </button>
                </>
              )}
            </p>
          ))}
          <button
            type="button"
            className="mero-cw-btn mero-cw-btn--primary mero-cw-btn--full"
            disabled={!flow.ready || flow.phase !== 'idle'}
            onClick={() => flow.accept(token)}
          >
            {flow.phase === 'creating' ? 'Joining…' : 'Join'}
          </button>
        </div>
      )}
    </DialogShell>
  );
}
