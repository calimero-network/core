import React from 'react';
import { useNavigate } from 'react-router-dom';
import ContentWrapper from '../components/login/ContentWrapper';
import LoginSelector from '../components/login/wallets/LoginSelector';

export default function AddRootKeyPage() {
  const navigate = useNavigate();
  console.log("testing wf");
  console.log("testing wf");
  return (
    <ContentWrapper>
      <LoginSelector
        navigateMetamaskLogin={() => navigate('/identity/root-key/metamask')}
        navigateNearLogin={() => navigate('/identity/root-key/near')}
        navigateStarknetLogin={() => navigate('/identity/root-key/starknet')}
        navigateIcpLogin={() => navigate('/identity/root-key/icp')}
      />
      <LoginSelector
        navigateMetamaskLogin={() => navigate('/identity/root-key/metamask')}
        navigateNearLogin={() => navigate('/identity/root-key/near')}
        navigateStarknetLogin={() => navigate('/identity/root-key/starknet')}
        navigateIcpLogin={() => navigate('/identity/root-key/icp')}
      />
      <LoginSelector
        navigateMetamaskLogin={() => navigate('/identity/root-key/metamask')}
        navigateNearLogin={() => navigate('/identity/root-key/near')}
        navigateStarknetLogin={() => navigate('/identity/root-key/starknet')}
        navigateIcpLogin={() => navigate('/identity/root-key/icp')}
      />
      <LoginSelector
        navigateMetamaskLogin={() => navigate('/identity/root-key/metamask')}
        navigateNearLogin={() => navigate('/identity/root-key/near')}
        navigateStarknetLogin={() => navigate('/identity/root-key/starknet')}
        navigateIcpLogin={() => navigate('/identity/root-key/icp')}
      />
    </ContentWrapper>
  );
}
