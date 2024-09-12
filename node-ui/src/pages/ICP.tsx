import React, { useEffect, useState } from 'react';
import ContentWrapper from '../components/login/ContentWrapper';
import { useNavigate } from 'react-router-dom';
import { ICP } from '../components/icp/ICP';
import { useICP } from '../hooks/useICP';

interface ICPProps {
  isLogin: boolean;
}

export default function ICPLogin({ isLogin }: ICPProps) {
  const navigate = useNavigate();
  const {
    ready,
    signMessageAndLogin,
    logout,
    walletSignatureData,
    requestNodeData,
    changeNetwork,
  } = useICP();
  const [errorMessage, setErrorMessage] = useState<string>('');

  useEffect(() => {
    if (!walletSignatureData) {
      requestNodeData({ setErrorMessage });
    }
  }, [requestNodeData, walletSignatureData]);

  return (
    <ContentWrapper>
      <ICP
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
