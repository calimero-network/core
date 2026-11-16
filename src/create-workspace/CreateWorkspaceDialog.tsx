import { useEffect, useId, useRef, useState, type ReactNode } from 'react';
import { createPortal } from 'react-dom';
import type { CalimeroClient, PasskeyProvider } from '@calimero-network/mero-js';
import { useMero } from '../context';
import { ApprovalCard, DialogShell, useMounted, WithClient } from '../consent-flow/DialogShell';
import { clusterName } from './clusterName';
import { WorkspaceReview } from './WorkspaceReview';
import { useCreateWorkspace, useCreateWorkspaceResume, type CreatedWorkspace } from './useCreateWorkspace';
import type { ReviewRelease, UnreleasedApp } from '../node-shield/nodeChecks';

/** The dialog's wording, so each app keeps its own. */
export interface CreateWorkspaceLabels {
  /** What is being created (`lobby`, `world`): the default sentences use it. Default `workspace`. */
  noun?: string;
  title?: string;
  subtitle?: string;
  nameLabel?: string;
  namePlaceholder?: string;
  selfLabel?: string;
  selfPlaceholder?: string;
  next?: string;
}

const NOUN = 'workspace';

/** The wording from `labels`: every default sentence follows `noun`, and a full label wins. */
function wording(labels?: CreateWorkspaceLabels) {
  const noun = labels?.noun ?? NOUN;
  return {
    noun,
    title: `Create ${noun}`,
    subtitle: `Name your ${noun}, and choose how others in it will see you.`,
    nameLabel: `${noun.charAt(0).toUpperCase()}${noun.slice(1)} name`,
    namePlaceholder: '',
    selfLabel: 'Your name',
    selfPlaceholder: '',
    next: 'Continue',
    ...labels,
  };
}

export interface CreateWorkspaceDialogProps {
  open: boolean;
  onClose: () => void;
  /**
   * The release the workspace will run, reviewed before creating. `null`
   * while the newest release is still being read; `{ package }` for a build
   * with no published release.
   */
  release: ReviewRelease | UnreleasedApp | null;
  /** Called once the workspace exists, from the dialog or from a full-page approval. The app does its own follow-ups. */
  onCreated: (workspace: CreatedWorkspace) => void;
  /** Where the node's consent page sends the person back. Default: this origin's root, where the app mounts `ConsentPopupReturn`. */
  returnTo?: string;
  labels?: CreateWorkspaceLabels;
  /** Extra inputs under the name fields; the app owns their state and reads it in `onCreated`. */
  fields?: ReactNode;
  /** Continue waits until this is true (default true): the validity of `fields`. */
  fieldsValid?: boolean;
  /** Prefills "your name" (an app's last used name). */
  defaultSelfName?: string;
  /** The label under "Where it runs". Default: the cluster named by the node in use. */
  cluster?: string | null;
  /** The app's own client, wrapped in a `MeroProvider` here. Without it the ambient `MeroProvider` is used. */
  client?: CalimeroClient;
  /** The passkey prompt, with `client`. */
  passkey?: PasskeyProvider;
}

/** Says why a full-page approval created nothing (declined, or a return that is not this tab's request). */
function ResumeNotice({ onCreated }: Pick<CreateWorkspaceDialogProps, 'onCreated'>) {
  const { message } = useCreateWorkspaceResume({ onCreated });
  const [dismissed, setDismissed] = useState<string | null>(null);
  const mounted = useMounted();
  if (!mounted || !message || dismissed === message) return null;
  return createPortal(
    <div role="alert" className="mero-cw mero-cw-notice">
      {message}{' '}
      <button type="button" className="mero-cw-link" onClick={() => setDismissed(message)}>
        Dismiss
      </button>
    </div>,
    document.body,
  );
}

/**
 * Create a workspace in three steps: names, review, and (on a session bound to
 * the app's frontend) approval on the node's own page. Mount it once under
 * sign-in and toggle `open`: it also finishes a workspace approved on a
 * full-page trip to the node. Needs a `MeroProvider`, or pass `client`.
 */
export function CreateWorkspaceDialog({ client, passkey, ...props }: CreateWorkspaceDialogProps) {
  return (
    <WithClient client={client} passkey={passkey}>
      <Dialog {...props} />
    </WithClient>
  );
}

function Dialog({ open, onClose, release, onCreated, returnTo, labels, defaultSelfName = '', cluster, fields, fieldsValid = true }: Omit<CreateWorkspaceDialogProps, 'client' | 'passkey'>) {
  const text = wording(labels);
  const { client, cluster: ambientCluster, nodeUrl } = useMero();
  const flow = useCreateWorkspace({ returnTo: returnTo ?? `${typeof window === 'undefined' ? '' : window.location.origin}/`, onCreated });
  const [step, setStep] = useState<'names' | 'review'>('names');
  const [name, setName] = useState('');
  const [selfName, setSelfName] = useState(defaultSelfName);
  const ids = [useId(), useId()];
  const first = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (open && release && step === 'names') first.current?.focus();
  }, [open, release, step]);
  const node = client?.nodeUrl ?? nodeUrl;
  const where = cluster !== undefined ? (cluster ?? undefined) : (ambientCluster ?? (node ? clusterName(node) : undefined));

  const trimmed = { name: name.trim(), self: selfName.trim() };
  const redirecting = flow.phase === 'redirect';
  const approving = flow.phase === 'approving' || redirecting;
  const body = !release ? (
    <p className="mero-cw-sub" role="status">
      Checking the newest release…
    </p>
  ) : approving ? (
    <ApprovalCard redirecting={redirecting} code={flow.code} onCancel={flow.cancel} onProceed={flow.proceed} onReopen={flow.reopen} />
  ) : step === 'names' ? (
    <form
      onSubmit={(e) => {
        e.preventDefault();
        if (trimmed.name && trimmed.self && fieldsValid) setStep('review');
      }}
    >
      <h2 className="mero-cw-title">{text.title}</h2>
      <p className="mero-cw-sub">{text.subtitle}</p>
      <div className="mero-cw-field">
        <label htmlFor={ids[0]}>{text.nameLabel}</label>
        <input id={ids[0]} ref={first} value={name} placeholder={text.namePlaceholder} onChange={(e) => setName(e.target.value)} />
      </div>
      <div className="mero-cw-field">
        <label htmlFor={ids[1]}>{text.selfLabel}</label>
        <input id={ids[1]} value={selfName} placeholder={text.selfPlaceholder} onChange={(e) => setSelfName(e.target.value)} />
      </div>
      {fields}
      <button type="submit" className="mero-cw-btn mero-cw-btn--primary mero-cw-btn--full" disabled={!trimmed.name || !trimmed.self || !fieldsValid}>
        {text.next}
      </button>
    </form>
  ) : (
    <>
      {flow.error && (
        <p className="mero-cw-error" role="alert">
          {flow.error}
        </p>
      )}
      <WorkspaceReview
        client={client}
        cluster={where}
        local={!!node && clusterName(node) === 'merod local'}
        noun={text.noun}
        release={release}
        creating={flow.phase === 'creating'}
        disabled={!flow.ready}
        onBack={() => setStep('names')}
        onCreate={() => flow.create(trimmed.name, trimmed.self)}
      />
    </>
  );

  return (
    <>
      <ResumeNotice onCreated={onCreated} />
      {open && (
        <DialogShell label={text.title} locked={approving} onClose={() => (approving ? flow.cancel() : onClose())}>
          {body}
        </DialogShell>
      )}
    </>
  );
}
