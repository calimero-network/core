import { useEffect, useId, useState } from 'react';
import type { CalimeroClient, PasskeyProvider } from '@calimero-network/mero-js';
import { ORG_CAPABILITIES } from '@calimero-network/mero-js';
import { useMero } from '../context';
import { ApprovalCard, DialogShell, WithClient } from '../consent-flow/DialogShell';
import { useCopy } from '../create-workspace/useCopy';
import { CAPABILITY_LABELS, describeCapabilities, heldCapabilities } from './capabilities';
import { useInviteCreate, useInviteResume, type CreatedInvite, type InviteRole } from './useInvite';

const FEATURE_NOT_ACTIVE = 'FEATURE_NOT_ACTIVE';

/** The dialog's wording, so each app keeps its own. */
export interface InviteLabels {
  title?: string;
  subtitle?: string;
  next?: string;
}

const DEFAULT_LABELS: Required<InviteLabels> = {
  title: 'Invite people',
  subtitle: 'Choose what the people you invite can do.',
  next: 'Create invite',
};

export interface InviteDialogProps {
  open: boolean;
  onClose: () => void;
  /** The workspace (organization) people are invited to. */
  org: string;
  /** The app's team(s) the invitee joins too, e.g. its app team. */
  teams?: string[];
  /** The role the invite gives. Default `member`. */
  role?: InviteRole;
  appName?: string;
  labels?: InviteLabels;
  /** Called once the invite exists, from the dialog or from a full-page approval. */
  onInvited?: (invite: CreatedInvite) => void;
  /** The page the link opens. Default: this page. */
  linkBase?: string;
  /** Where the node's consent page sends the person back. Default: this origin's root, where the app mounts `ConsentPopupReturn`. */
  returnTo?: string;
  /** The app's own client, wrapped in a `MeroProvider` here. Without it the ambient `MeroProvider` is used. */
  client?: CalimeroClient;
  /** The passkey prompt, with `client`. */
  passkey?: PasskeyProvider;
}

type OrgFacts = { defaults: number; held: number } | 'unreadable' | null;

/** What the inviter may grant and what a default invite gives, read from the workspace. */
function useOrgFacts(org: string): OrgFacts {
  const { client, accountId } = useMero();
  const [facts, setFacts] = useState<OrgFacts>(null);
  useEffect(() => {
    if (!client) return;
    let live = true;
    setFacts(null);
    client.orgs.get({ org }).then(
      (info) => live && setFacts({ defaults: info.defaultCapabilities, held: heldCapabilities(info, accountId) }),
      () => live && setFacts('unreadable'),
    );
    return () => {
      live = false;
    };
  }, [client, org, accountId]);
  return facts;
}

const originRoot = () => `${typeof window === 'undefined' ? '' : window.location.origin}/`;

/**
 * Invite people to a workspace: choose their permissions (the workspace's
 * defaults, or a custom set the inviter holds), create the invite (through the
 * node's consent page for a session bound to the app), then share the link.
 * Mount it once and toggle `open`: it also finishes an invite approved on a
 * full-page trip to the node. Needs a `MeroProvider`, or pass `client`.
 */
export function InviteDialog({ client, passkey, ...props }: InviteDialogProps) {
  return (
    <WithClient client={client} passkey={passkey}>
      <Dialog {...props} />
    </WithClient>
  );
}

function Dialog({ open, onClose, org, teams, role = 'member', appName, labels, onInvited, linkBase, returnTo }: Omit<InviteDialogProps, 'client' | 'passkey'>) {
  const text = { ...DEFAULT_LABELS, ...labels };
  const facts = useOrgFacts(org);
  const [mode, setMode] = useState<'default' | 'custom'>('default');
  const [picked, setPicked] = useState<number | null>(null);
  const [shared, setShared] = useState<CreatedInvite | null>(null);
  const [copied, copy] = useCopy();
  const ids = [useId(), useId()];
  const onCreated = (invite: CreatedInvite) => {
    setShared(invite);
    onInvited?.(invite);
  };
  const flow = useInviteCreate({ returnTo: returnTo ?? originRoot(), linkBase, onCreated });
  const resume = useInviteResume({ linkBase, onCreated });
  const [dismissed, setDismissed] = useState<string | null>(null);

  const readable = facts !== null && facts !== 'unreadable';
  const bits = picked ?? (readable ? facts.defaults & facts.held : 0);
  const redirecting = flow.phase === 'redirect';
  const approving = flow.phase === 'approving' || redirecting;
  const notice = resume.message && dismissed !== resume.message ? resume.message : null;
  const featureOff = flow.errorCode === FEATURE_NOT_ACTIVE;
  const close = () => {
    setShared(null);
    onClose();
  };

  if (!open && !shared && !notice) return null;

  let body;
  if (approving) {
    body = <ApprovalCard redirecting={redirecting} code={flow.code} onCancel={flow.cancel} onProceed={flow.proceed} onReopen={flow.reopen} />;
  } else if (shared) {
    const perms = shared.capabilities === undefined ? null : describeCapabilities(shared.capabilities);
    body = (
      <div>
        <h2 className="mero-cw-title">Invite link ready</h2>
        <p className="mero-cw-sub">
          Anyone with this link can join as a {role}
          {appName ? ` in ${appName}` : ''}.{' '}
          {perms ? (perms.length ? `They can: ${perms.join(', ')}.` : 'They get basic access only.') : "They get the workspace's member defaults."}
        </p>
        <div className="mero-cw-field">
          <label htmlFor={ids[0]}>Invite link</label>
          <input id={ids[0]} readOnly value={shared.link} onFocus={(e) => e.currentTarget.select()} />
        </div>
        <div className="mero-cw-row">
          <button type="button" className="mero-cw-btn mero-cw-btn--primary" onClick={() => copy(shared.link)}>
            Copy link
          </button>
          <span className="mero-cw-wait" role="status">
            {copied === 'done' ? 'Copied' : copied === 'failed' ? 'Could not copy' : ''}
          </span>
          <button type="button" className="mero-cw-btn" onClick={close}>
            Done
          </button>
        </div>
      </div>
    );
  } else {
    const defaultWords = !readable ? "the workspace's member defaults" : describeCapabilities(facts.defaults).join(', ') || 'basic access only';
    body = (
      <div>
        <h2 className="mero-cw-title">{text.title}</h2>
        <p className="mero-cw-sub">{text.subtitle}</p>
        {notice && (
          <p className="mero-cw-error" role="alert">
            {notice}{' '}
            <button type="button" className="mero-cw-link" onClick={() => setDismissed(notice)}>
              Dismiss
            </button>
          </p>
        )}
        {flow.error && (
          <p className="mero-cw-error" role="alert">
            {featureOff ? "This cluster doesn't support custom permissions yet. Send a default invite instead." : flow.error}
            {featureOff && (
              <>
                {' '}
                <button
                  type="button"
                  className="mero-cw-link"
                  onClick={() => {
                    setMode('default');
                    flow.clearError();
                  }}
                >
                  Send a default invite
                </button>
              </>
            )}
          </p>
        )}
        {facts === 'unreadable' && <p className="mero-cw-sub">We could not read this workspace's permissions, so only a default invite is available.</p>}
        <fieldset className="mero-cw-choices">
          <legend className="mero-cw-visually-hidden">Permissions</legend>
          <label className="mero-cw-choice">
            <input type="radio" name={ids[1]} checked={mode === 'default'} onChange={() => setMode('default')} />
            <span>
              <b>Default</b>
              <span className="mero-cw-sub">{`They get the workspace's member defaults: ${defaultWords}.`}</span>
            </span>
          </label>
          <label className="mero-cw-choice">
            <input type="radio" name={ids[1]} checked={mode === 'custom'} disabled={!readable} onChange={() => setMode('custom')} />
            <span>
              <b>Custom</b>
              <span className="mero-cw-sub">Choose exactly what they can do.</span>
            </span>
          </label>
        </fieldset>
        {mode === 'custom' && readable && (
          <div className="mero-cw-caps">
            {CAPABILITY_LABELS.map(({ name, label }) => {
              const bit = ORG_CAPABILITIES[name];
              const mine = (facts.held & bit) !== 0;
              return (
                <div key={name} className="mero-cw-cap" data-disabled={mine ? undefined : ''}>
                  <label>
                    <input type="checkbox" checked={mine && (bits & bit) !== 0} disabled={!mine} aria-describedby={mine ? undefined : `${ids[1]}-${name}`} onChange={(e) => setPicked(e.target.checked ? bits | bit : bits & ~bit)} />
                    {label}
                  </label>
                  {!mine && (
                    <span id={`${ids[1]}-${name}`} className="mero-cw-hint">
                      You don't have this
                    </span>
                  )}
                </div>
              );
            })}
          </div>
        )}
        <button
          type="button"
          className="mero-cw-btn mero-cw-btn--primary mero-cw-btn--full"
          disabled={!flow.ready || flow.phase !== 'idle' || (mode === 'custom' && !readable)}
          onClick={() => flow.create({ org, role, teams, ...(mode === 'custom' ? { capabilities: bits } : {}) })}
        >
          {flow.phase === 'creating' ? 'Creating…' : text.next}
        </button>
      </div>
    );
  }

  return (
    <DialogShell label={text.title} locked={approving} onClose={() => (approving ? flow.cancel() : close())}>
      {body}
    </DialogShell>
  );
}
