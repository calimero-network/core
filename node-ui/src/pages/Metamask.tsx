import React, { useEffect, useState } from 'react';
import ContentWrapper from '../components/login/ContentWrapper';
import { useMetamask } from '../hooks/useMetamask';
import { MetamaskWallet } from '../components/metamask/MetamaskWallet';
import { useNavigate } from 'react-router-dom';
import { getAppEndpointKey } from '../utils/storage';

interface MetamaskLoginProps {
  isLogin: boolean;
}

export default function MetamaskLogin({ isLogin }: MetamaskLoginProps) {
  const navigate = useNavigate();
  const {
    ready,
    isConnected,
    address,
    walletSignatureData,
    isSignSuccess,
    isSignLoading,
    signMessage,
    isSignError,
    requestNodeData,
    login,
    signData,
  } = useMetamask();
  const [errorMessage, setErrorMessage] = useState<string>('');

  useEffect(() => {
    if (isConnected) {
      requestNodeData({ setErrorMessage });
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [isConnected]);

  useEffect(() => {
    getAppEndpointKey() || navigate('/?node_error=true');
    if (isSignSuccess && walletSignatureData) {
      login({ isLogin, setErrorMessage });
    }
  }, [address, isSignSuccess, isLogin, login, signData, walletSignatureData]);

  return (
    <ContentWrapper>
      <MetamaskWallet
        navigateBack={() =>
          isLogin ? navigate('/auth') : navigate('/identity/root-key')
        }
        ready={ready}
        isConnected={isConnected}
        walletSignatureData={walletSignatureData}
        isSignLoading={isSignLoading}
        signMessage={signMessage}
        isSignError={isSignError}
        errorMessage={errorMessage}
        isLogin={isLogin}
      />
    </ContentWrapper>
  );
}
