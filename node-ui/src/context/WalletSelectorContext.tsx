import type { ReactNode } from 'react';
import React, {
  useCallback,
  useContext,
  useEffect,
  useState,
  useMemo,
} from 'react';

import type {
  AccountState,
  NetworkId,
  WalletSelector,
} from '@near-wallet-selector/core';
import { setupWalletSelector } from '@near-wallet-selector/core';
import type { WalletSelectorModal } from '@near-wallet-selector/modal-ui';
import { setupModal } from '@near-wallet-selector/modal-ui';
import { setupMyNearWallet } from '@near-wallet-selector/my-near-wallet';
import Loading from '../components/common/Loading';
import translations from '../constants/en.global.json';

const t = translations.walletSelectorContext;

declare global {
  export interface Window {
    // @ts-expect-error
    selector: WalletSelector;
    modal: WalletSelectorModal;
  }
}

export interface WalletSelectorContextValue {
  selector: WalletSelector;
  modal: WalletSelectorModal;
  accounts: Array<AccountState>;
  accountId: string | null;
}

const WalletSelectorContext =
  React.createContext<WalletSelectorContextValue | null>(null);

interface WalletSelectorContextProviderProps {
  network: string;
  children: ReactNode;
}

export function WalletSelectorContextProvider({
  network,
  children,
}: WalletSelectorContextProviderProps) {
  const [selector, setSelector] = useState<WalletSelector | null>(null);
  const [modal, setModal] = useState<WalletSelectorModal | null>(null);
  const [accounts, setAccounts] = useState<Array<AccountState>>([]);
  const [loading, setLoading] = useState<boolean>(true);

  const init = useCallback(async () => {
    const _selector = await setupWalletSelector({
      network: network as NetworkId,
      debug: true,
      modules: [setupMyNearWallet()],
    });
    const _modal = setupModal(_selector, {
      contractId: '',
    });
    const state = _selector.store.getState();
    setAccounts(state.accounts);

    window.selector = _selector;
    window.modal = _modal;

    setSelector(_selector);
    setModal(_modal);
    setLoading(false);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    init().catch((err) => {
      console.error(err);
      alert(t.alertErrorText);
    });
  }, [init]);

  const walletSelectorContextValue = useMemo<WalletSelectorContextValue>(
    () => ({
      selector: selector!,
      modal: modal!,
      accounts,
      accountId: accounts.find((account) => account.active)?.accountId || null,
    }),
    [selector, modal, accounts],
  );

  if (loading) {
    return <Loading />;
  }

  return (
    <WalletSelectorContext.Provider value={walletSelectorContextValue}>
      {children}
    </WalletSelectorContext.Provider>
  );
}

export function useWalletSelector() {
  const context = useContext(WalletSelectorContext);

  if (!context) {
    throw new Error(t.componentErrorText);
  }

  return context;
}
