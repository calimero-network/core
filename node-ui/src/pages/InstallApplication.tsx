import React, { useState } from 'react';
import { Navigation } from '../components/Navigation';
import { FlexLayout } from '../components/layout/FlexLayout';
import PageContentWrapper from '../components/common/PageContentWrapper';
import { useNavigate } from 'react-router-dom';
import { ContentCard } from '../components/common/ContentCard';
import translations from '../constants/en.global.json';
import InstallApplicationCard from '../components/applications/InstallApplicationCard';
import { apiClient } from '@calimero-network/calimero-client';

export interface Application {
  appId: string;
  name: string;
  version: string;
  path: string;
  hash: string;
}

export default function InstallApplication() {
  const t = translations.applicationsPage.installApplication;
  const navigate = useNavigate();
  const [application, setApplication] = useState<Application>({
    appId: '',
    name: '',
    version: '',
    path: '',
    hash: '',
  });
  const [showBrowseApplication, setShowBrowseApplication] = useState(false);
  const [isLoading, setIsLoading] = useState(false);
  const [showStatusModal, setShowStatusModal] = useState(false);
  const [installAppStatus, setInstallAppStatus] = useState({
    title: '',
    message: '',
    error: false,
  });

  const installApplication = async () => {
    setIsLoading(true);
    const appId: string | null = await installApplicationHandler();
    if (!appId) {
      setIsLoading(false);
      setShowStatusModal(true);
      return;
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
      setInstallAppStatus({
        title: t.failInstallTitle,
        message: response.error.message,
        error: true,
      });
      return null;
    } else {
      setInstallAppStatus({
        title: t.successInstallTitle,
        message: `Installed application ${application.name}, version ${application.version}.`,
        error: false,
      });
      return response.data.applicationId;
    }
  };

  const closeModal = () => {
    setShowStatusModal(false);
    if (installAppStatus.error) {
      setInstallAppStatus({
        title: '',
        message: '',
        error: false,
      });
      return;
    }
    navigate('/applications');
  };

  return (
    <FlexLayout>
      <Navigation />
      <PageContentWrapper>
        <ContentCard
          headerBackText={t.backButtonText}
          headerOnBackClick={() => navigate('/applications')}
        >
          <InstallApplicationCard
            application={application}
            setApplication={setApplication}
            installApplication={installApplication}
            showBrowseApplication={showBrowseApplication}
            setShowBrowseApplication={setShowBrowseApplication}
            onUploadClick={() => navigate('/publish-application')}
            isLoading={isLoading}
            showStatusModal={showStatusModal}
            closeModal={closeModal}
            installAppStatus={installAppStatus}
          />
        </ContentCard>
      </PageContentWrapper>
    </FlexLayout>
  );
}
