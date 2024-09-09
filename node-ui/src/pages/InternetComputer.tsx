import React, { useEffect, useState } from 'react';
import ContentWrapper from '../components/login/ContentWrapper';
import { useNavigate } from 'react-router-dom';
import { InternetComputer } from '../components/internetComputer/internetComputer';
import { useInternetComputer } from '../hooks/useInternetComputer';

interface InternetComputerProps {
  isLogin: boolean;
}

export default function InternetComputerLogin({ isLogin }: InternetComputerProps) {
  const navigate = useNavigate();
  const {
    ready,
    signMessageAndLogin,
    logout,
    walletSignatureData,
    requestNodeData,
    changeNetwork,
  } = useInternetComputer();
  const [errorMessage, setErrorMessage] = useState<string>('');

  useEffect(() => {
    if (!walletSignatureData) {
      requestNodeData({ setErrorMessage });
    }
  }, [requestNodeData, walletSignatureData]);

  return (
    <ContentWrapper>
      <InternetComputer
        navigateBack={() =>
          isLogin ? navigate('/auth') : navigate('/identity/root-key')
        }
        isLogin={isLogin}
        ready={ready}
        walletSignatureData={walletSignatureData}
        signMessageAndLogin={signMessageAndLogin}
        logout={logout}
        setErrorMessage={setErrorMessage}
        errorMessage={errorMessage}
        changeNetwork={changeNetwork}
      />
    </ContentWrapper>
  );
}
