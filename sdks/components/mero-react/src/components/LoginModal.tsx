/**
 * LoginModal: sign in, or create an account, with a passkey.
 *
 * On a page the node serves (`servedByNode`) the node is the page's origin, so
 * the dialog asks nothing but the passkey. Elsewhere it asks first for the
 * node's URL or a Calimero Cloud cluster's name (`eu-shared-1`). Creating an account asks for a name (what the passkey is saved
 * under).
 */

import React, { useEffect, useId, useRef, useState } from 'react';
import { useMero } from '../context';
import { resolveMeroTheme, themeToCssVars, type MeroTheme } from '../theme';
import { CalimeroLogo } from './CalimeroLogo';

export type LoginModalTab = 'sign-in' | 'sign-up';

/** A cluster's name as the node directory knows it: no scheme, no dots. */
const CLUSTER_NAME = /^[a-z0-9][a-z0-9-]{0,62}$/i;

export interface LoginModalProps {
  isOpen: boolean;
  onClose: () => void;
  /** Called once a session is open. */
  onSignedIn?: () => void;
  /** Which form to open on. Default `sign-in`. */
  initialTab?: LoginModalTab;
  /** Offer account creation. Default true. */
  allowSignUp?: boolean;
  /** The app's name, shown above the title. */
  appName?: string;
  theme?: MeroTheme;
}

export function LoginModal({
  isOpen,
  onClose,
  onSignedIn,
  initialTab = 'sign-in',
  allowSignUp = true,
  appName,
  theme,
}: LoginModalProps) {
  const {
    servedByNode,
    nodeUrl,
    cluster,
    client,
    connectToNode,
    connectToCluster,
    signIn,
    signUp,
    isLoading,
    isConnecting,
    failure,
  } = useMero();
  const [tab, setTab] = useState<LoginModalTab>(initialTab);
  const [url, setUrl] = useState(cluster ?? nodeUrl ?? '');
  const [name, setName] = useState('');
  const [localError, setLocalError] = useState<string | null>(null);
  // A sign-in waiting for the client of a node just chosen.
  const [queued, setQueued] = useState<LoginModalTab | null>(null);
  const titleId = useId();
  const dialogRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (isOpen) {
      setTab(initialTab);
      setLocalError(null);
      setUrl(cluster ?? nodeUrl ?? '');
      dialogRef.current?.focus();
    }
    // Reset only when it opens.
  }, [isOpen]);

  const run = async (which: LoginModalTab) => {
    const session = which === 'sign-up' ? await signUp(name.trim()) : await signIn();
    if (session) {
      onSignedIn?.();
      onClose();
    }
  };

  useEffect(() => {
    if (!queued || !client) return;
    const target = url.trim().replace(/\/+$/, '');
    // A cluster's client exists only once the directory named its nodes.
    if (cluster ? cluster !== target : client.nodeUrl.replace(/\/+$/, '') !== target) return;
    const which = queued;
    setQueued(null);
    void run(which);
  }, [queued, client]);

  // The directory could not name the cluster's nodes: stop waiting, show why.
  useEffect(() => {
    if (queued && failure && !isConnecting) setQueued(null);
  }, [queued, failure, isConnecting]);

  if (!isOpen) return null;

  const submit = (event: React.FormEvent) => {
    event.preventDefault();
    setLocalError(null);
    if (tab === 'sign-up' && !name.trim()) {
      setLocalError('Enter a name for your account.');
      return;
    }
    if (!servedByNode) {
      const target = url.trim();
      if (CLUSTER_NAME.test(target)) {
        if (!client || target !== cluster) {
          connectToCluster(target);
          setQueued(tab);
          return;
        }
      } else {
        try {
          const parsed = new URL(target);
          if (parsed.protocol !== 'https:' && parsed.protocol !== 'http:') throw new Error();
        } catch {
          setLocalError('Enter the node URL (https://node.example.com) or a cluster name (eu-shared-1).');
          return;
        }
        if (!client || cluster || target.replace(/\/+$/, '') !== nodeUrl) {
          connectToNode(target);
          setQueued(tab);
          return;
        }
      }
    }
    void run(tab);
  };

  const vars = theme ? themeToCssVars(resolveMeroTheme(theme)) : undefined;
  const message = localError ?? failure?.message ?? null;
  const busy = isLoading || queued !== null;

  return (
    <div
      className="mero-dialog-overlay"
      style={vars}
      onMouseDown={(e) => {
        if (e.target === e.currentTarget && !busy) onClose();
      }}
    >
      <div
        ref={dialogRef}
        className="mero-dialog-panel"
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        tabIndex={-1}
        onKeyDown={(e) => {
          if (e.key === 'Escape' && !busy) onClose();
        }}
      >
        {appName && <p className="mero-dialog-app">{appName}</p>}
        <div className="mero-dialog-head">
          <span className="mero-dialog-icon" aria-hidden="true">
            <CalimeroLogo size={20} />
          </span>
          <h2 id={titleId} className="mero-dialog-title">
            {tab === 'sign-up' ? 'Create your account' : 'Sign in'}
          </h2>
        </div>
        <p className="mero-dialog-body">
          {tab === 'sign-up'
            ? 'Your device saves a passkey for this account. There is no password.'
            : 'Use the passkey saved on this device.'}
        </p>
        <form onSubmit={submit} noValidate>
          {!servedByNode && (
            <label className="mero-dialog-field">
              Node URL or cluster
              <input
                className="mero-dialog-input"
                type="text"
                inputMode="url"
                autoComplete="url"
                autoCapitalize="none"
                spellCheck={false}
                placeholder="https://node.example.com or eu-shared-1"
                value={url}
                onChange={(e) => setUrl(e.target.value)}
                disabled={busy}
              />
            </label>
          )}
          {tab === 'sign-up' && (
            <label className="mero-dialog-field">
              Name
              <input
                className="mero-dialog-input"
                type="text"
                autoComplete="username webauthn"
                value={name}
                onChange={(e) => setName(e.target.value)}
                disabled={busy}
              />
            </label>
          )}
          {message && (
            <p className="mero-dialog-body mero-dialog-body-error" role="alert">
              {message}
            </p>
          )}
          {busy && (
            <div className="mero-dialog-progress" aria-hidden="true">
              <div className="mero-dialog-progress-bar" />
            </div>
          )}
          <div className="mero-dialog-actions">
            <button type="button" className="mero-dialog-button mero-dialog-secondary" onClick={onClose} disabled={busy}>
              Cancel
            </button>
            <button type="submit" className="mero-dialog-button mero-dialog-primary" disabled={busy}>
              {tab === 'sign-up' ? 'Create account' : 'Sign in with passkey'}
            </button>
          </div>
        </form>
        {allowSignUp && (
          <button
            type="button"
            className="mero-dialog-link"
            onClick={() => {
              setLocalError(null);
              setTab(tab === 'sign-up' ? 'sign-in' : 'sign-up');
            }}
            disabled={busy}
          >
            {tab === 'sign-up' ? 'I already have a passkey' : 'New here? Create an account'}
          </button>
        )}
      </div>
    </div>
  );
}

export default LoginModal;
