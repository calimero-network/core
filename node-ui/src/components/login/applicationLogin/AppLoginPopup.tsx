import React, { useEffect, useState } from 'react';
import Modal from 'react-bootstrap/Modal';
import apiClient from '../../../api';
import {
  Context,
  ContextIdentitiesResponse,
  ContextList,
  CreateTokenResponse,
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

interface AppLoginPopupProps {
  showPopup: boolean;
  callbackUrl: string;
  applicationId: string;
  showServerDownPopup: () => void;
}

export const enum LoginStep {
  SELECT_CONTEXT,
  SELECT_IDENTITY,
  CREATE_ACCESS_TOKEN,
}

export default function AppLoginPopup({
  showPopup,
  callbackUrl,
  applicationId,
  showServerDownPopup,
}: AppLoginPopupProps) {
  const [contextList, setContextList] = useState<Context[]>([]);
  const [contextIdentities, setContextIdentities] = useState<string[]>([]);
  const [selectedIdentity, setSelectedIdentity] = useState('');
  const [selectedContextId, setSelectedContextId] = useState('');
  const [loginStep, setLoginStep] = useState(LoginStep.SELECT_CONTEXT);
  const [errorMessage, setErrorMessage] = useState('');
  const t = translations.appLoginPopup;

  useEffect(() => {
    const fetchAvailableContexts = async () => {
      const fetchContextsResponse: ResponseData<ContextList> = await apiClient(
        showServerDownPopup,
      )
        .node()
        .getContexts();
      const contexts =
        fetchContextsResponse.data?.contexts.filter(
          (context) => context.applicationId === applicationId,
        ) ?? [];
      setContextList(contexts);
    };
    fetchAvailableContexts();
  }, [showPopup, applicationId, showServerDownPopup]);

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
      {loginStep === LoginStep.SELECT_CONTEXT && (
        <SelectContextStep
          applicationId={applicationId}
          callbackUrl={callbackUrl}
          contextList={contextList}
          selectedContextId={selectedContextId}
          setSelectedContextId={setSelectedContextId}
          updateLoginStep={() => setLoginStep(LoginStep.SELECT_IDENTITY)}
          finishLogin={finishLogin}
        />
      )}
      {loginStep === LoginStep.SELECT_IDENTITY && (
        <SelectIdentityStep
          applicationId={applicationId}
          callbackUrl={callbackUrl}
          contextIdentities={contextIdentities}
          selectedIdentity={selectedIdentity}
          setSelectedIdentity={setSelectedIdentity}
          updateLoginStep={() => setLoginStep(LoginStep.CREATE_ACCESS_TOKEN)}
          finishLogin={finishLogin}
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
        />
      )}
    </Modal>
  );
}
