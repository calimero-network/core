/**
 * ConnectButton: "Sign in" when signed out (opens `LoginModal`), the account
 * when signed in, with a menu to sign out.
 */

import React, { useEffect, useMemo, useRef, useState } from 'react';
import { useMero } from '../context';
import { resolveMeroTheme, themeToCssVars, type MeroTheme } from '../theme';
import { CalimeroLogo } from './CalimeroLogo';
import { LoginModal, type LoginModalTab } from './LoginModal';

export interface ConnectButtonProps {
  className?: string;
  style?: React.CSSProperties;
  theme?: MeroTheme;
  /** Only the logo (a 40×40 icon button); the label stays for screen readers. */
  logoOnly?: boolean;
  /** Labels; a string is shorthand for `{ connect }`. */
  label?: string | { connect?: string; connected?: string };
  /** The app's name, shown in the sign-in dialog. */
  appName?: string;
  /** Offer account creation in the dialog. Default true. */
  allowSignUp?: boolean;
  /** Which form the dialog opens on. */
  initialTab?: LoginModalTab;
}

/** `abcdef…123456`: enough of an account id to recognise it. */
export function shortAccount(account: string): string {
  return account.length > 14 ? `${account.slice(0, 6)}…${account.slice(-6)}` : account;
}

export function ConnectButton({
  className,
  style,
  theme,
  logoOnly = false,
  label,
  appName,
  allowSignUp,
  initialTab,
}: ConnectButtonProps) {
  const { isAuthenticated, accountId, nodeUrl, cluster, signOut, signOutEverywhere, servedByNode, allowMock } =
    useMero();
  const [isModalOpen, setIsModalOpen] = useState(false);
  const [isMenuOpen, setIsMenuOpen] = useState(false);
  const containerRef = useRef<HTMLDivElement>(null);

  const themeVars = useMemo(() => (theme ? themeToCssVars(resolveMeroTheme(theme)) : undefined), [theme]);
  const labels = useMemo(() => {
    const overrides = typeof label === 'string' ? { connect: label } : label;
    return { connect: overrides?.connect ?? 'Sign in', connected: overrides?.connected ?? null };
  }, [label]);

  useEffect(() => {
    if (!isMenuOpen) return;
    const close = (event: MouseEvent) => {
      if (containerRef.current && !containerRef.current.contains(event.target as Node)) setIsMenuOpen(false);
    };
    document.addEventListener('mousedown', close);
    return () => document.removeEventListener('mousedown', close);
  }, [isMenuOpen]);

  const classes = (...extra: Array<string | false | undefined>) =>
    ['mero-connect-button', logoOnly && 'mero-logo-only', ...extra, className].filter(Boolean).join(' ');

  if (isAuthenticated) {
    const text = labels.connected ?? (accountId ? shortAccount(accountId) : 'Signed in');
    return (
      <div ref={containerRef} className="mero-connect-container" style={{ ...themeVars, position: 'relative', display: 'inline-block' }}>
        <button
          className={classes('mero-connected')}
          style={style}
          onClick={() => setIsMenuOpen((open) => !open)}
          aria-haspopup="menu"
          aria-expanded={isMenuOpen}
          aria-label={text}
          title={logoOnly ? text : accountId ?? undefined}
        >
          <CalimeroLogo size={18} className="mero-logo" />
          {!logoOnly && text}
        </button>
        {isMenuOpen && (
          <div className="mero-dropdown" role="menu">
            {accountId && (
              <div className="mero-dropdown-info" title={accountId} data-testid="connected-account">
                Account {shortAccount(accountId)}
              </div>
            )}
            {!servedByNode && (cluster || nodeUrl) && (
              <div className="mero-dropdown-info" title={nodeUrl ?? undefined}>
                {cluster ? `Cluster ${cluster}` : nodeUrl}
                {allowMock ? ' (dev)' : ''}
              </div>
            )}
            <button
              className="mero-dropdown-item"
              role="menuitem"
              onClick={() => {
                setIsMenuOpen(false);
                void signOut();
              }}
            >
              Sign out
            </button>
            <button
              className="mero-dropdown-item"
              role="menuitem"
              onClick={() => {
                setIsMenuOpen(false);
                void signOutEverywhere();
              }}
            >
              Sign out everywhere
            </button>
          </div>
        )}
      </div>
    );
  }

  return (
    <div className="mero-connect-container" style={{ ...themeVars, display: 'inline-block' }}>
      <button
        className={classes()}
        style={style}
        onClick={() => setIsModalOpen(true)}
        aria-label={labels.connect}
        title={logoOnly ? labels.connect : undefined}
      >
        <CalimeroLogo size={18} className="mero-logo" />
        {!logoOnly && labels.connect}
      </button>
      <LoginModal
        isOpen={isModalOpen}
        onClose={() => setIsModalOpen(false)}
        theme={theme}
        appName={appName}
        allowSignUp={allowSignUp}
        initialTab={initialTab}
      />
    </div>
  );
}

export default ConnectButton;
