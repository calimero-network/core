import React, { useEffect, useState } from 'react';
import { Navigation } from '../components/Navigation';
import { FlexLayout } from '../components/layout/FlexLayout';
import { useNavigate } from 'react-router-dom';
import translations from '../constants/en.global.json';
import JoinContextCard from '../components/context/joinContext/JoinContextCard';
import styled from 'styled-components';
import { ModalContent } from '../components/common/StatusModal';
import { apiClient, ResponseData } from '@calimero-network/calimero-client';

export interface ContextApplication {
  appId: string;
  name: string;
  version: string;
}

const Wrapper = styled.div`
  display: flex;
  width: 100%;
  padding: 4.705rem 2rem 2rem;
  font-optical-sizing: auto;
  font-weight: 500;
  font-style: normal;
  font-variation-settings: 'slnt' 0;
  -webkit-font-smoothing: antialiased;
  -moz-osx-font-smoothing: grayscale;
  font-smooth: never;
`;

export default function JoinContextPage() {
  const t = translations.joinContextPage;
  const navigate = useNavigate();
  const [contextId, setContextId] = useState('');
  const [showModal, setShowModal] = useState(false);
  const [modalContent, setModalContent] = useState<ModalContent>({
    title: '',
    message: '',
    error: false,
  });

  const handleJoinContext = async () => {
    // TODO: Implement join context
    // const fetchApplicationResponse = await apiClient.node().joinContext(contextId);
    // if (fetchApplicationResponse.error) {
    //   setModalContent({
    //     title: t.joinErrorTitle,
    //     message: fetchApplicationResponse.error.message,
    //     error: true,
    //   });
    //   setShowModal(true);
    //   return;
    // }
    // setModalContent({
    //   title: t.joinSuccessTitle,
    //   message: t.joinSuccessMessage,
    //   error: false,
    // });
    // setShowModal(true);
  };

  const closeModal = () => {
    setShowModal(false);
    if (!modalContent.error) {
      setContextId('');
      setModalContent({
        title: '',
        message: '',
        error: false,
      });
      navigate('/contexts');
    }
  };

  useEffect(() => {
    navigate('/contexts');
  }, [navigate]);

  return (
    <FlexLayout>
      <Navigation />
      <Wrapper>
        <JoinContextCard
          handleJoinContext={handleJoinContext}
          contextId={contextId}
          setContextId={setContextId}
          showModal={showModal}
          modalContent={modalContent}
          closeModal={closeModal}
          navigateBack={() => navigate('/contexts')}
        />
      </Wrapper>
    </FlexLayout>
  );
}
