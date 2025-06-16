import React, { useState } from 'react';
import { Navigation } from '../components/Navigation';
import { FlexLayout } from '../components/layout/FlexLayout';
import PageContentWrapper from '../components/common/PageContentWrapper';
import { useNavigate } from 'react-router-dom';
import { ContentCard } from '../components/common/ContentCard';
import StartContextCard from '../components/context/startContext/StartContextCard';
import translations from '../constants/en.global.json';
import { apiClient } from '@calimero-network/calimero-client';

export interface ContextApplication {
  appId: string;
  name: string;
  version: string;
  path: string;
  hash: string;
}

export default function StartContextPage() {
  const t = translations.startContextPage;
  const navigate = useNavigate();
  const [application, setApplication] = useState<ContextApplication>({
    appId: '',
    name: '',
    version: '',
    path: '',
    hash: '',
  });
  const [isArgsChecked, setIsArgsChecked] = useState(false);
  const [argumentsJson, setArgumentsJson] = useState('');
  const [showBrowseApplication, setShowBrowseApplication] = useState(false);
  const [isLoading, setIsLoading] = useState(false);
  const [showStatusModal, setShowStatusModal] = useState(false);
  const [protocol, setProtocol] = useState('');
  const [startContextStatus, setStartContextStatus] = useState({
    title: '',
    message: '',
    error: false,
  });

  const startContext = async () => {
    setIsLoading(true);
    const appId: string | null = await installApplicationHandler();
    if (!appId) {
      setIsLoading(false);
      setShowStatusModal(true);
      return;
    }
    const startContextResponse = await apiClient
      .node()
      .createContext(appId, protocol);
    if (startContextResponse.error) {
      setStartContextStatus({
        title: t.startContextErrorTitle,
        message: t.startContextErrorMessage,
        error: true,
      });
    } else {
      setStartContextStatus({
        title: t.startContextSuccessTitle,
        message: t.startedContextMessage,
        error: false,
      });
    }
    setIsLoading(false);
    setShowStatusModal(true);
  };

  const installApplicationHandler = async (): Promise<string | null> => {
    if (!application.appId || !application.version) {
      return null;
    }

    const response = await apiClient
      .node()
      .installApplication(
        application.appId,
        application.version,
        application.path,
        application.hash,
      );
    if (response.error) {
      setStartContextStatus({
        title: t.failInstallTitle,
        message: response.error.message,
        error: true,
      });
      return null;
    } else {
      setStartContextStatus({
        title: t.successInstallTitle,
        message: `Installed application ${application.name}, version ${application.version}.`,
        error: false,
      });
      return response.data.applicationId;
    }
  };

  const closeModal = () => {
    setShowStatusModal(false);
    if (startContextStatus.error) {
      setStartContextStatus({
        title: '',
        message: '',
        error: false,
      });
      return;
    }
    navigate('/contexts');
  };

  return (
    <FlexLayout>
      <Navigation />
      <PageContentWrapper>
        <ContentCard
          headerBackText={t.backButtonText}
          headerOnBackClick={() => navigate('/contexts')}
        >
          <StartContextCard
            application={application}
            setApplication={setApplication}
            isArgsChecked={isArgsChecked}
            setIsArgsChecked={setIsArgsChecked}
            argumentsJson={argumentsJson}
            setArgumentsJson={setArgumentsJson}
            setProtocol={setProtocol}
            startContext={startContext}
            showBrowseApplication={showBrowseApplication}
            setShowBrowseApplication={setShowBrowseApplication}
            onUploadClick={() => navigate('/publish-application')}
            isLoading={isLoading}
            showStatusModal={showStatusModal}
            closeModal={closeModal}
            startContextStatus={startContextStatus}
          />
        </ContentCard>
      </PageContentWrapper>
    </FlexLayout>
  );
}
