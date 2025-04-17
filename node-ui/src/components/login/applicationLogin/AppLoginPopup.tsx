import React, { useEffect, useState } from 'react';
import Modal from 'react-bootstrap/Modal';
import apiClient from '../../../api';
import {
  Context,
  ContextIdentitiesResponse,
  CreateTokenResponse,
  GetContextsResponse,
  GetInstalledApplicationsResponse,
} from '../../../api/dataSource/NodeDataSource';
import { ResponseData } from '../../../api/response';
import SelectContextStep from './SelectContextStep';
import CreateAccessTokenStep from './CreateAccessTokenStep';
import SelectIdentityStep from './SelectIdentityStep';
import {
  setStorageApplicationId,
  setStorageCallbackUrl,
} from '../../../auth/storage';
import translations from '../../../constants/en.global.json';
import StartContextStep from './StartContextStep';

interface AppLoginPopupProps {
  showPopup: boolean;
  callbackUrl: string;
  applicationId: string;
  showServerDownPopup: () => void;
  closePopup: () => void;
}

export const enum LoginStep {
  SELECT_CONTEXT,
  SELECT_IDENTITY,
  CREATE_ACCESS_TOKEN,
  START_NEW_CONTEXT,
}

export default function AppLoginPopup({
  showPopup,
  callbackUrl,
  applicationId,
  showServerDownPopup,
  closePopup,
}: AppLoginPopupProps) {
  const [applicationError, setApplicationError] = useState('');
  const [contextList, setContextList] = useState<Context[]>([]);
  const [contextIdentities, setContextIdentities] = useState<string[]>([]);
  const [selectedIdentity, setSelectedIdentity] = useState('');
  const [selectedContextId, setSelectedContextId] = useState('');
  const [loginStep, setLoginStep] = useState(LoginStep.SELECT_CONTEXT);
  const [errorMessage, setErrorMessage] = useState('');
  const t = translations.appLoginPopup;

  useEffect(() => {
    const fetchApplication = async () => {
      const fetchApplicationResponse: ResponseData<GetInstalledApplicationsResponse> =
        await apiClient(showServerDownPopup).node().getInstalledApplications();
      const applications = fetchApplicationResponse.data;
      const installedApplication = applications?.apps.find(
        (app) => app.id === applicationId,
      );
      if (installedApplication) {
        setApplicationError('');
      } else {
        setApplicationError(t.applicationError);
      }
    };
    fetchApplication();
  }, [applicationId]);

  useEffect(() => {
    const fetchAvailableContexts = async () => {
      const fetchContextsResponse: ResponseData<GetContextsResponse> =
        await apiClient(showServerDownPopup).node().getContexts();
      const contexts =
        fetchContextsResponse.data?.contexts.filter(
          (context) => context.applicationId === applicationId,
        ) ?? [];
      setContextList(contexts);
    };
    fetchAvailableContexts();
  }, [showPopup, applicationId, showServerDownPopup, loginStep]);

  useEffect(() => {
    const fetchAvailableContextIdentities = async () => {
      const fetchContextIdentitiesResponse: ResponseData<ContextIdentitiesResponse> =
        await apiClient(showServerDownPopup)
          .node()
          .getContextIdentity(selectedContextId);
      const identities = fetchContextIdentitiesResponse.data?.identities ?? [];
      setContextIdentities(identities);
    };
    if (selectedContextId) {
      fetchAvailableContextIdentities();
    }
  }, [selectedContextId, showServerDownPopup]);

  const finishLogin = (accessTokens?: string) => {
    if (!accessTokens) {
      window.location.href = callbackUrl;
      return;
    }
    try {
      const tokenData = JSON.parse(accessTokens);
      const { access_token, refresh_token } = tokenData;
      const encodedAccessToken = encodeURIComponent(access_token);
      const encodedRefreshToken = encodeURIComponent(refresh_token);
      const newUrl = `${callbackUrl}?access_token=${encodedAccessToken}&refresh_token=${encodedRefreshToken}`;
      window.location.href = newUrl;
    } catch (error) {
      console.error('Error parsing access token:', error);
      window.location.href = callbackUrl;
    }
  };

  const onCreateToken = async () => {
    try {
      setErrorMessage('');
      setStorageApplicationId('');
      setStorageCallbackUrl('');
      const createTokenResponse: ResponseData<CreateTokenResponse> =
        await apiClient(showServerDownPopup)
          .node()
          .createAccessToken(selectedContextId, selectedIdentity);
      const accessTokens = createTokenResponse.data;
      if (accessTokens) {
        finishLogin(JSON.stringify(accessTokens));
      } else {
        setErrorMessage(t.createTokenError);
      }
    } catch (e) {
      console.log(e);
      setErrorMessage(t.createTokenError);
    }
  };

  return (
    <Modal
      show={showPopup}
      backdrop="static"
      keyboard={false}
      aria-labelledby="contained-modal-title-vcenter"
      centered
    >
      {loginStep === LoginStep.START_NEW_CONTEXT && (
        <StartContextStep
          applicationId={applicationId}
          updateLoginStep={() => setLoginStep(LoginStep.SELECT_CONTEXT)}
          backLoginStep={() => setLoginStep(LoginStep.SELECT_CONTEXT)}
        />
      )}
      {loginStep === LoginStep.SELECT_CONTEXT && (
        <SelectContextStep
          applicationId={applicationId}
          callbackUrl={callbackUrl}
          contextList={contextList}
          setSelectedContextId={setSelectedContextId}
          updateLoginStep={() => setLoginStep(LoginStep.SELECT_IDENTITY)}
          createContext={() => {
            setSelectedContextId('');
            setLoginStep(LoginStep.START_NEW_CONTEXT);
          }}
          applicationError={applicationError}
          closePopup={closePopup}
        />
      )}
      {loginStep === LoginStep.SELECT_IDENTITY && (
        <SelectIdentityStep
          applicationId={applicationId}
          callbackUrl={callbackUrl}
          selectedContextId={selectedContextId}
          contextIdentities={contextIdentities}
          updateLoginStep={(selectedIdentity: string) => {
            setSelectedIdentity(selectedIdentity);
            setLoginStep(LoginStep.CREATE_ACCESS_TOKEN);
          }}
          backLoginStep={() => setLoginStep(LoginStep.SELECT_CONTEXT)}
          closePopup={closePopup}
        />
      )}
      {loginStep === LoginStep.CREATE_ACCESS_TOKEN && (
        <CreateAccessTokenStep
          applicationId={applicationId}
          callbackUrl={callbackUrl}
          selectedContextId={selectedContextId}
          selectedIdentity={selectedIdentity}
          onCreateToken={onCreateToken}
          errorMessage={errorMessage}
          backLoginStep={() => setLoginStep(LoginStep.SELECT_IDENTITY)}
          closePopup={closePopup}
        />
      )}
    </Modal>
  );
}
