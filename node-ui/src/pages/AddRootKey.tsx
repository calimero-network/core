import React from 'react';
import { LoginSelector } from '@calimero-is-near/calimero-p2p-sdk';
import { useNavigate } from 'react-router-dom';
import ContentWrapper from '../components/login/ContentWrapper';

export default function AddRootKeyPage() {
  const navigate = useNavigate();
  return (
    <ContentWrapper>
      <LoginSelector
        navigateMetamaskLogin={() => navigate('/identity/root-key/metamask')}
        navigateNearLogin={() => navigate('/identity/root-key/near')}
        cardBackgroundColor={undefined}
      />
    </ContentWrapper>
  );
}
