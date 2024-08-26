import React, { useEffect, useState } from 'react';
import Modal from 'react-bootstrap/Modal';
import apiClient from '../../../api';
import { Context, ContextList } from '../../../api/dataSource/NodeDataSource';
import { ResponseData } from '../../../api/response';
import SelectContextStep from './SelectContextStep';
import CreateAccessTokenStep from './CreateAccessTokenStep';

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
  const [selectedContextId, setSelectedContextId] = useState('');
  const [loginStep, setLoginStep] = useState(LoginStep.SELECT_CONTEXT);

  useEffect(() => {
    const fetchAvailableContexts = async () => {
      const fetchContextsResponse: ResponseData<ContextList> = await apiClient(
        showServerDownPopup,
      )
        .node()
        .getContexts();
      console.log(fetchContextsResponse.data?.contexts);
      const contexts =
        fetchContextsResponse.data?.contexts.filter(
          (context) => context.applicationId === applicationId,
        ) ?? [];
      setContextList(contexts);
    };
    fetchAvailableContexts();
  }, [showPopup, applicationId, showServerDownPopup]);

  const finishLogin = () => {
    window.location.href = callbackUrl;
  };

  const onCreateToken = async () => {
    // TBD
    // const createTokenResponse = await apiClient(showServerDownPopup)
    //   .node()
    //   .createAccessToken(applicationId, selectedContextId);
    // if (createTokenResponse.success) {
    //   finishLogin();
    // }
  }

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
          updateLoginStep={() => setLoginStep(LoginStep.CREATE_ACCESS_TOKEN)}
        />
      )}
      {loginStep === LoginStep.CREATE_ACCESS_TOKEN && (
        <CreateAccessTokenStep
          applicationId={applicationId}
          callbackUrl={callbackUrl}
          selectedContextId={selectedContextId}
          onCreateToken={onCreateToken}
        />
      )}
    </Modal>
  );
}
