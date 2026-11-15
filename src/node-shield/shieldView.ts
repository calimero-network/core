/** What the node shield says about a node, from the check mero-js ran: only what that check verified. */
import { verifyNodeUrl, type NodeVerificationState } from '@calimero-network/mero-js';

const SEALED_PROFILE = 'locked-read-only'; // the release profile that seals a node's data
const JUST_NOW_SECONDS = 10;
const AGO = [
  ['day', 86_400],
  ['hour', 3_600],
  ['minute', 60],
] as const;
const ago = new Intl.RelativeTimeFormat('en');

/** Whether the node runs the app's code: `match` only when both hashes are present and equal; anything not checked is `unknown`. */
export type AppCodeCheck =
  | { status: 'match'; appHash: string; checkedAt: number; package?: string; version?: string }
  | { status: 'mismatch'; appHash: string; expected: string }
  | { status: 'unknown'; reason: string };

export type ShieldKind = 'checking' | 'verified' | 'unsealed' | 'refused';

export interface ShieldRow {
  label: string;
  value: string;
  /** What `value` abbreviates, for the copied details. */
  full?: string;
  mono?: boolean;
  tone?: 'bad';
  /** The short hash that ends `value`, kept on one line. */
  hash?: string;
}

export interface ShieldView {
  kind: ShieldKind;
  /** The chip's text. */
  label: string;
  title: string;
  sub: string;
  rows: ShieldRow[];
  /** Facts the check verified for this node, and nothing else. */
  checks: string[];
  /** The public verifier's page for this node; `null` unless a signed release judged a real quote. */
  verifyUrl: string | null;
}

const SHIELD_LABEL: Record<ShieldKind, string> = {
  checking: 'Checking node…',
  verified: 'Verified node',
  unsealed: 'Test server: not sealed',
  refused: 'Node refused',
};

/** Sealed means the locked profile on a real quote; `unreachable` is no answer yet, so it reads as checking. */
export function shieldKind(state: NodeVerificationState): ShieldKind {
  if (state.status === 'refused') return 'refused';
  if (state.status !== 'verified') return 'checking';
  return !state.mock && state.profile === SEALED_PROFILE ? 'verified' : 'unsealed';
}

/** "2 minutes ago", or "just now" for the last few seconds. */
export function checkedAgo(at: number, now: number = Date.now()): string {
  const seconds = Math.max(0, Math.round((now - at) / 1000));
  if (seconds < JUST_NOW_SECONDS) return 'just now';
  const [unit, size] = AGO.find(([, size]) => seconds >= size) ?? (['second', 1] as const);
  return ago.format(-Math.floor(seconds / size), unit);
}

/** `7f3a…c91e`. */
export function shortHash(hash: string): string {
  return hash.length > 12 ? `${hash.slice(0, 4)}…${hash.slice(-4)}` : hash;
}

function hostOf(url: string): string {
  try {
    return new URL(url).host;
  } catch {
    return url;
  }
}

function releaseName(code: Extract<AppCodeCheck, { status: 'match' }>): string | null {
  return code.package && code.version ? `${code.package} v${code.version}` : null;
}

function codeRow(code: AppCodeCheck | null): ShieldRow | null {
  if (code?.status === 'match') {
    const named = releaseName(code);
    return {
      label: 'App code',
      value: named ? `${named} · ${shortHash(code.appHash)}` : shortHash(code.appHash),
      full: named ? `${named} · ${code.appHash}` : code.appHash,
      mono: true,
      hash: shortHash(code.appHash),
    };
  }
  if (code?.status === 'mismatch') {
    return {
      label: 'App code',
      value: `Does not match this app's release (node runs ${shortHash(code.appHash)})`,
      full: `Does not match this app's release (node runs ${code.appHash}, expected ${code.expected})`,
      tone: 'bad',
    };
  }
  return null;
}

function copy(state: NodeVerificationState, kind: ShieldKind, ago: string): Pick<ShieldView, 'title' | 'sub'> {
  switch (kind) {
    case 'verified':
      return {
        title: 'You are talking to a genuine Calimero TEE node',
        sub: `Checked by this browser ${ago}, before anything was sent.`,
      };
    case 'unsealed':
      return {
        title: SHIELD_LABEL.unsealed,
        sub:
          state.status === 'verified' && state.mock
            ? `A test server with no hardware enclave, so its data is not sealed. Checked by this browser ${ago}.`
            : `This node did not show it runs the sealed, read-only profile, so its data is not sealed. Checked by this browser ${ago}.`,
      };
    case 'refused':
      return { title: SHIELD_LABEL.refused, sub: 'This app refused the node and sent nothing.' };
    case 'checking':
      return {
        title: SHIELD_LABEL.checking,
        sub:
          state.status === 'unreachable'
            ? 'The node could not be reached. Trying again.'
            : 'This browser is checking the node before anything is sent.',
      };
  }
}

/** Everything the chip and popover show for `state`, in `cluster`, with the app's code check. */
export function describeShield(
  state: NodeVerificationState,
  { cluster, code, now = Date.now() }: { cluster?: string | null; code: AppCodeCheck | null; now?: number },
): ShieldView {
  const kind = shieldKind(state);
  // A mock is never a real node: nothing about releases or app code is claimed for one.
  const real = state.status === 'verified' && !state.mock ? state : null;
  const release = real?.release;
  const rows: ShieldRow[] = [];
  if (cluster) rows.push({ label: 'Cluster', value: cluster });
  rows.push({ label: 'Node', value: hostOf(state.url), full: state.url, mono: true });
  if (real && release) rows.push({ label: 'Software', value: `mero-tee v${release}${real.profile ? ` · ${real.profile}` : ''}` });
  const app = real ? codeRow(code) : null;
  if (app) rows.push(app);
  if (state.status === 'refused' || state.status === 'unreachable') {
    rows.push({ label: 'Reason', value: state.reason, ...(state.status === 'refused' ? { tone: 'bad' as const } : {}) });
  }
  const checked = 'checkedAt' in state ? checkedAgo(state.checkedAt, now) : null;
  if (checked) rows.push({ label: 'Checked', value: checked });

  const checks: string[] = [];
  if (kind === 'verified') checks.push('Running in an Intel TDX enclave (not debug)');
  if (real && release) checks.push(`Measurements match the signed v${release} release`);
  if (real) checks.push("Your connection is encrypted to this enclave's key");
  if (app && code?.status === 'match') {
    const named = releaseName(code);
    checks.push(named ? `Node runs the code listed for ${named}` : 'Node runs the code bundle this app was built for');
  }

  return {
    kind,
    label: SHIELD_LABEL[kind],
    ...copy(state, kind, checked ?? 'just now'),
    rows,
    checks,
    verifyUrl: real && release ? verifyNodeUrl(real.url, release) : null,
  };
}

/** The popover's facts as plain text, for "Copy details". */
export function shieldDetails(view: ShieldView): string {
  return [
    view.title,
    view.sub,
    '',
    ...view.rows.map((r) => `${r.label}: ${r.full ?? r.value}`),
    ...(view.checks.length ? ['', ...view.checks.map((c) => `- ${c}`)] : []),
  ].join('\n');
}
