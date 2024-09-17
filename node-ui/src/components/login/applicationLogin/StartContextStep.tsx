import React, { useEffect, useState } from 'react';
import apiClient from '../../../api';
import { useServerDown } from '../../../context/ServerDownContext';
import translations from '../../../constants/en.global.json';
import { useRPC } from '../../../hooks/useNear';
import { Package, Release } from '../../../pages/Applications';
import { styled } from 'styled-components';
import StartContextPopup from './StartContextPopup';

export const ModalWrapper = styled.div`
  display: flex;
  flex-direction: column;
  justify-content: center;
  padding: 1.5rem;
  border-radius: 0.375rem;
  items-align: center;
  background-color: #17191b;

  .title {
    font-size: 1.25rem;
    font-weight: 700;
    line-height: 2rem;
    color: #fff;
    text-align: center;
  }
`;

interface StartContextStepProps {
  applicationId: string;
  updateLoginStep: () => void;
  backLoginStep: () => void;
}

export interface DisplayApplication {
  appId: string;
  name: string;
  version: string;
}

export default function StartContextStep({
  applicationId,
  updateLoginStep,
  backLoginStep,
}: StartContextStepProps) {
  const t = translations.startContextPage;
  const { getLatestRelease, getPackage } = useRPC();
  const [isLoading, setIsLoading] = useState(false);
  const [startFinished, setStartFinished] = useState(false);
  const [isArgsChecked, setIsArgsChecked] = useState(false);
  const [argumentsJson, setArgumentsJson] = useState('');
  const { showServerDownPopup } = useServerDown();
  const [application, setApplication] = useState<DisplayApplication | null>(
    null,
  );
  const [startContextStatus, setStartContextStatus] = useState({
    title: '',
    message: '',
    error: false,
  });

  const startContext = async () => {
    setIsLoading(true);
    if (!applicationId) {
      setIsLoading(false);
      setStartFinished(true);
      return;
    }
    const startContextResponse = await apiClient(showServerDownPopup)
      .node()
      .startContexts(applicationId, argumentsJson);
    if (startContextResponse.error) {
      setStartContextStatus({
        title: t.startContextErrorTitle,
        message: t.startContextErrorMessage,
        error: true,
      });
    } else {
      updateLoginStep();
    }
    setIsLoading(false);
    setStartFinished(true);
  };

  useEffect(() => {
    const setApplications = async () => {
      const packageMetadata: Package | null = await getPackage(applicationId);
      let releaseData: Release | null = null;
      if (packageMetadata) {
        releaseData = await getLatestRelease(applicationId);
      }
      setApplication({
        appId: applicationId,
        name: packageMetadata?.name ?? '-',
        version: releaseData?.version ?? '-',
      });
    };
    if (!application) {
      setApplications();
    }
  }, [getLatestRelease, getPackage, applicationId, application]);

  return (
    <ModalWrapper>
      <div className="title">{t.loginRequestPopupTitle}</div>
      <StartContextPopup
        application={application}
        isArgsChecked={isArgsChecked}
        setIsArgsChecked={setIsArgsChecked}
        argumentsJson={argumentsJson}
        setArgumentsJson={setArgumentsJson}
        startContext={startContext}
        isLoading={isLoading}
        showStatusModal={startFinished}
        closeModal={() => setStartFinished(false)}
        startContextStatus={startContextStatus}
        backLoginStep={backLoginStep}
      />
    </ModalWrapper>
  );
}
