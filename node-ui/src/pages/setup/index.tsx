import React from 'react';
import { useNavigate } from 'react-router-dom';
import { setAppEndpointKey } from '../../utils/storage';
import ContentWrapper from '../../components/login/ContentWrapper';
import { SetupModal } from '../../components/setup/SetupModal';
import { getNodeUrl } from '../../utils/node';

export default function SetupPage() {
  const navigate = useNavigate();

  return (
    <ContentWrapper>
      <SetupModal
        successRoute={() => navigate('/auth')}
        setNodeUrl={setAppEndpointKey}
        getNodeUrl={getNodeUrl}
      />
    </ContentWrapper>
  );
}
