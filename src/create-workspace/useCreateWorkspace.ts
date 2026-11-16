import { createOrgConsent } from '@calimero-network/mero-js';
import type { ConsentResult } from '@calimero-network/mero-js';
import { useCreateOrg } from '../hooks/orgs';
import { useConsentResume, useNodeConsent, type NodeConsentPhase } from '../consent-flow/useNodeConsent';

/** What the app is told once the workspace exists. */
export interface CreatedWorkspace {
  orgId: string;
  /** The workspace's name, as the person typed it. */
  name: string;
  selfName: string;
}

export type CreatePhase = NodeConsentPhase;

const INTENT_KEY = 'mero-react:create-workspace-intent';
const NO_ORG = 'Your node approved the request but named no workspace. Nothing was created.';

interface Intent {
  name: string;
  selfName: string;
}

const isIntent = (v: unknown): v is Intent =>
  !!v && typeof (v as Intent).name === 'string' && typeof (v as Intent).selfName === 'string';

/** The new org's id from an approved result. */
const orgIdOf = (result: ConsentResult) => (result.status === 'approved' && result.kind === 'createOrg' ? result.outcome?.orgId : undefined);

export interface UseCreateWorkspaceOptions {
  /** Where the node's consent page sends the person back: a page of the app that mounts `ConsentPopupReturn`. */
  returnTo: string;
  onCreated: (workspace: CreatedWorkspace) => void;
}

/**
 * Creates a workspace. A session bound to the app's frontend asks the node's
 * consent page (in a pop-up, else a full-page trip after `proceed`); any other
 * session creates the organization directly. Resolves through `onCreated`;
 * `error` is the node's own message, or what happened to the approval.
 */
export function useCreateWorkspace({ returnTo, onCreated }: UseCreateWorkspaceOptions) {
  const orgs = useCreateOrg({ throwOnError: true });
  const { start, ...flow } = useNodeConsent<Intent>({ storageKey: INTENT_KEY });
  const create = (name: string, selfName: string) =>
    start({
      request: createOrgConsent(name, returnTo),
      intent: { name, selfName },
      direct: () => orgs.create(name),
      done: (outcome) => {
        const orgId = 'direct' in outcome ? outcome.direct : orgIdOf(outcome.consent);
        if (!orgId) return NO_ORG;
        onCreated({ orgId, name, selfName });
        return undefined;
      },
    });
  return { create, ...flow };
}

/**
 * Finishes a workspace the person approved on a full-page trip to the node:
 * call it where the app starts, with the same `onCreated`. `message` says why
 * nothing was created (declined, or a return that is not this tab's request).
 */
export function useCreateWorkspaceResume({ onCreated }: Pick<UseCreateWorkspaceOptions, 'onCreated'>) {
  return useConsentResume<Intent>({
    storageKey: INTENT_KEY,
    valid: isIntent,
    onApproved: (intent, result) => {
      const orgId = orgIdOf(result);
      if (!orgId) return NO_ORG;
      onCreated({ orgId, name: intent.name, selfName: intent.selfName });
      return undefined;
    },
  });
}
