import React, { useEffect, useState } from 'react';
import { useNavigate } from 'react-router-dom';

import Loading from '../components/common/Loading';
import { useWalletSelector } from '../context/WalletSelectorContext';
import { useNear, useWallet } from '../hooks/useNear';

import '@near-wallet-selector/modal-ui/styles.css';
import ContentWrapper from '../components/login/ContentWrapper';
import NearWallet, { Account } from '../components/near/NearWallet';
import { useServerDown } from '../context/ServerDownContext';
import { getAppEndpointKey } from '../utils/storage';

export interface Message {
  premium: boolean;
  sender: string;
  text: string;
}

interface NearLoginProps {
  isLogin: boolean;
}

export default function NearLogin({ isLogin }: NearLoginProps) {
  const navigate = useNavigate();
  const { showServerDownPopup } = useServerDown();
  const { selector, accounts, modal, accountId } = useWalletSelector();
  const { getAccount, handleSignMessage, verifyMessageBrowserWallet } = useNear(
    {
      accountId,
      selector,
    },
  );
  const { handleSwitchWallet, handleSwitchAccount, handleSignOut } =
    useWallet();
  const [account, setAccount] = useState<Account | null>(null);
  const [loading, setLoading] = useState<boolean>(false);
  const [errorMessage, setErrorMessage] = useState('');
  const appName = 'me';

  useEffect(() => {
    getAppEndpointKey() || navigate('/');
    const timeoutId = setTimeout(() => {
      verifyMessageBrowserWallet(isLogin, setErrorMessage, showServerDownPopup);
    }, 500);

    return () => {
      clearTimeout(timeoutId);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    if (!accountId) {
      return setAccount(null);
    }
    setLoading(true);
    getAccount().then((nextAccount: any) => {
      setAccount(nextAccount);
      setLoading(false);
    });
  }, [accountId, getAccount]);

  if (loading) {
    return (
      <ContentWrapper>
        <Loading />
      </ContentWrapper>
    );
  }

  return (
    <ContentWrapper>
      <NearWallet
        isLogin={isLogin}
        navigateBack={() =>
          isLogin ? navigate('/auth') : navigate('/identity/root-key')
        }
        account={account}
        accounts={accounts}
        errorMessage={errorMessage}
        handleSignout={() =>
          handleSignOut({
            account,
            selector,
            setAccount,
            setErrorMessage,
          })
        }
        handleSwitchWallet={() => handleSwitchWallet(modal)}
        handleSignMessage={() => {
          getAppEndpointKey() || navigate('/');
          handleSignMessage({
            selector,
            appName,
            setErrorMessage,
            showServerDownPopup,
          });
        }}
        handleSwitchAccount={() =>
          handleSwitchAccount({ accounts, accountId, selector })
        }
      />
    </ContentWrapper>
  );
}
