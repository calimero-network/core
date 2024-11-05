import React, { useEffect, useState } from 'react';
import ContentWrapper from '../components/login/ContentWrapper';
import { useNavigate } from 'react-router-dom';
import { StarknetWallet } from '../components/starknet/StarknetWallet';
import { useStarknet } from '../hooks/useStarknet';
import { getAppEndpointKey } from '../utils/storage';

interface StarknetLoginProps {
  isLogin: boolean;
}

export default function StarknetLogin({ isLogin }: StarknetLoginProps) {
  const navigate = useNavigate();
  const {
    ready,
    walletLogin,
    starknetInstance,
    argentXId,
    signData,
    signMessage,
    logout,
    walletSignatureData,
    requestNodeData,
    login,
    changeMetamaskNetwork,
  } = useStarknet();
  const [errorMessage, setErrorMessage] = useState<string>('');

  useEffect(() => {
    if (starknetInstance) {
      requestNodeData({ setErrorMessage });
    }
  }, [starknetInstance, requestNodeData]);

  useEffect(() => {
    getAppEndpointKey() || navigate('/');
    if (starknetInstance && signData) {
      login({ isLogin, setErrorMessage });
    }
  }, [login, signData, isLogin, starknetInstance]);

  return (
    <ContentWrapper>
      <StarknetWallet
        navigateBack={() =>
          isLogin ? navigate('/auth') : navigate('/identity/root-key')
        }
        isLogin={isLogin}
        ready={ready}
        walletLogin={walletLogin}
        starknetInstance={starknetInstance}
        argentXId={argentXId}
        walletSignatureData={walletSignatureData}
        signMessage={signMessage}
        logout={logout}
        setErrorMessage={setErrorMessage}
        errorMessage={errorMessage}
        changeMetamaskNetwork={changeMetamaskNetwork}
      />
    </ContentWrapper>
  );
}
