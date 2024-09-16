import React, { useEffect, useState } from 'react';
import ContentWrapper from '../components/login/ContentWrapper';
import { useNavigate } from 'react-router-dom';
import { Icp } from '../components/icp/Icp';
import { useIcp } from '../hooks/useIcp';

interface IcpProps {
  isLogin: boolean;
}

export default function IcpLogin({ isLogin }: IcpProps) {
  const navigate = useNavigate();
  const {
    ready,
    signMessageAndLogin,
    logout,
    walletSignatureData,
    requestNodeData,
    changeNetwork,
  } = useIcp();
  const [errorMessage, setErrorMessage] = useState<string>('');

  useEffect(() => {
    if (!walletSignatureData) {
      requestNodeData({ setErrorMessage });
    }
  }, [requestNodeData, walletSignatureData]);

  return (
    <ContentWrapper>
      <Icp
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
