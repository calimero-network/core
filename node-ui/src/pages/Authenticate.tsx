import React from 'react';
import { useNavigate } from 'react-router-dom';
import ContentWrapper from '../components/login/ContentWrapper';
import LoginSelector from '../components/login/wallets/LoginSelector';

export default function AuthenticatePage() {
  const navigate = useNavigate();
  return (
    <ContentWrapper>
      <LoginSelector
        navigateMetamaskLogin={() => navigate('/auth/metamask')}
        navigateNearLogin={() => navigate('/auth/near')}
        navigateStarknetLogin={() => navigate('/auth/starknet')}
        navigateIcpLogin={() => navigate('/auth/icp')}
      />
    </ContentWrapper>
  );
}
