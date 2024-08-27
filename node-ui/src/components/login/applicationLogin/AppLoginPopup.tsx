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
      console.log(identities);
      setContextIdentities(identities);
    };
    if (selectedContextId) {
      fetchAvailableContextIdentities();
    }
  }, [selectedContextId, showServerDownPopup]);

  const finishLogin = (accessToken?: string) => {
    if (!accessToken) {
      window.location.href = callbackUrl;
      return;
    }
    try {
      const tokenData = JSON.parse(accessToken);
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
    const createTokenResponse: ResponseData<CreateTokenResponse> =
      await apiClient(showServerDownPopup)
        .node()
        .createAccessToken(selectedContextId, selectedIdentity);
    const accessToken = createTokenResponse.data?.jwt_token;
    if (accessToken) {
      finishLogin(JSON.stringify(accessToken));
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
        />
      )}
    </Modal>
  );
}
