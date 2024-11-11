import React, { useEffect } from 'react';
import { useNavigate } from 'react-router-dom';
import { clearStorage, setAppEndpointKey } from '../../utils/storage';
import ContentWrapper from '../../components/login/ContentWrapper';
import { SetupModal } from '../../components/setup/SetupModal';
import { getNodeUrl } from '../../utils/node';

export default function SetupPage() {
  const navigate = useNavigate();

  useEffect(() => {
    clearStorage();
  }, []);

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
