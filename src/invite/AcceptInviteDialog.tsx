import { useEffect, useState } from 'react';
import type { CalimeroClient, PasskeyProvider } from '@calimero-network/mero-js';
import { useMero } from '../context';
import { ApprovalCard, DialogShell, WithClient } from '../consent-flow/DialogShell';
import { useAcceptResume, useInviteAccept, type JoinedInvite } from './useInvite';

/** The dialog's wording, so each app keeps its own. */
export interface AcceptInviteLabels {
  /** What the invite is to (`game`, `table`): the default sentences use it. Default `workspace`. */
  noun?: string;
  title?: string;
  body?: string;
  join?: string;
}

export interface AcceptInviteDialogProps {
  open: boolean;
  onClose: () => void;
  /** The invite token from the link (`readInviteToken`, or `useInviteRedemption`'s `token`). */
  token: string;
  /**
   * The org the link names, when it does. An account already in it is told so,
   * and the invite is not redeemed (a redeem spends a use even for a member).
   */
  org?: string;
  appName?: string;
  labels?: AcceptInviteLabels;
  /** Called when the node refuses the join or the request, with the error itself (to clear a spent stored token, say). A declined approval is not an error. */
  onError?: (error: unknown) => void;
  /** Called once the person has joined (`alreadyMember: false`), or, with `org`, chose to continue as a member (`alreadyMember: true`; nothing was redeemed), from the dialog or from a full-page approval. */
  onJoined: (joined: JoinedInvite) => void;
  /** Where the node's consent page sends the person back. Default: this origin's root, where the app mounts `ConsentPopupReturn`. */
  returnTo?: string;
  client?: CalimeroClient;
  passkey?: PasskeyProvider;
}

type Membership = 'checking' | 'no' | { name: string };

/** Whether the signed-in account is in `org` (and its name there); unknown counts as not, so the invite is asked as for a link that names no org. */
function useMembership(org: string | undefined, active: boolean): Membership {
  const { client } = useMero();
  const [state, setState] = useState<{ org: string; value: Membership } | null>(null);
  useEffect(() => {
    if (!org || !active || !client) return;
    let live = true;
    client.auth.me().then(
      (me) => {
        const found = (me as { orgs?: Array<{ orgId?: string; name?: string }> }).orgs?.find((o) => o.orgId === org);
        if (live) setState({ org, value: found ? { name: found.name ?? '' } : 'no' });
      },
      () => live && setState({ org, value: 'no' }),
    );
    return () => {
      live = false;
    };
  }, [client, org, active]);
  if (!org) return 'no';
  return state?.org === org ? state.value : 'checking';
}

const article = (noun: string) => (/^[aeiou]/i.test(noun) ? 'an' : 'a');

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

function Dialog({ open, onClose, token, org, appName, labels, onJoined, onError, returnTo }: Omit<AcceptInviteDialogProps, 'client' | 'passkey'>) {
  const noun = labels?.noun ?? 'workspace';
  const text = {
    title: labels?.title ?? `Join ${noun}`,
    body: labels?.body ?? `You were invited to ${article(noun)} ${noun}${appName ? ` in ${appName}` : ''}. Join it with your account.`,
    join: labels?.join ?? 'Join',
  };
  const flow = useInviteAccept({ returnTo: returnTo ?? `${typeof window === 'undefined' ? '' : window.location.origin}/`, noun, onJoined, onError });
  const resume = useAcceptResume({ noun, onJoined });
  const member = useMembership(org, open);
  const [dismissed, setDismissed] = useState<string | null>(null);
  const notice = resume.message && dismissed !== resume.message ? resume.message : null;
  const redirecting = flow.phase === 'redirect';
  const approving = flow.phase === 'approving' || redirecting;
  if (!open && !notice) return null;
  const already = typeof member === 'object' ? member : null;
  const name = already?.name || `this ${noun}`;
  let body;
  if (approving) {
    body = <ApprovalCard redirecting={redirecting} code={flow.code} onCancel={flow.cancel} onProceed={flow.proceed} onReopen={flow.reopen} />;
  } else if (already && org) {
    body = (
      <div>
        <h2 className="mero-cw-title">{`You're already in ${name}`}</h2>
        <p className="mero-cw-sub">{`The invitation was for ${name}, and you are already a member, so there is nothing to accept. It is still unused for the person it was meant for.`}</p>
        <button type="button" className="mero-cw-btn mero-cw-btn--primary mero-cw-btn--full" onClick={() => onJoined({ org, alreadyMember: true })}>
          Continue
        </button>
      </div>
    );
  } else {
    body = (
      <div>
        <h2 className="mero-cw-title">{text.title}</h2>
        <p className="mero-cw-sub">{text.body}</p>
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
          disabled={!flow.ready || flow.phase !== 'idle' || member === 'checking'}
          onClick={() => flow.accept(token)}
        >
          {flow.phase === 'creating' ? 'Joining…' : text.join}
        </button>
      </div>
    );
  }
  return (
    <DialogShell label={already ? `You're already in ${name}` : text.title} locked={approving} onClose={() => (approving ? flow.cancel() : onClose())}>
      {body}
    </DialogShell>
  );
}
