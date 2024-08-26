import React, { useEffect, useState } from 'react';
import Modal from 'react-bootstrap/Modal';
import { styled } from 'styled-components';
import apiClient from '../../../api';
import { ContextList } from '../../../api/dataSource/NodeDataSource';
import { ResponseData } from '../../../api/response';

interface AppLoginPopupProps {
  showPopup: boolean;
  callbackUrl: string;
  applicationId: string;
  showServerDownPopup: () => void;
}

const ModalWrapper = styled.div`
  display: flex;
  flex-direction: column;
  justify-content: center;
  padding: 1.5rem;
  border-radius: 0.375rem;
  items-align: center;
  background-color: #17191b;

  .title {
    text-align: center;
    font-size: 1rem;
    font-weight: 500;
    line-height: 1.25rem;
    color: #fff;
  }

  .subtitle {
    margin-top: 1.25rem;
    color: #fff;
    font-weigth: 500;
    font-size: 0.875rem;
  }

  .context-list {
    margin-top: 1.25rem;
    color: #fff;
  }

  .app-id {
    color: #ff7a00;
  }

  .app-callbackurl {
    color: #ff7a00;
    text-decoration: none;
`;

export default function AppLoginPopup({
  showPopup,
  callbackUrl,
  applicationId,
  showServerDownPopup
}: AppLoginPopupProps) {
  const [contextList, setContextList] = useState<string[]>([]);
  const finishLogin = () => {
    window.location.href = callbackUrl;
  };

  useEffect(() => {
    const fetchAvailableContexts = async () => {
      const fetchContextsResponse: ResponseData<ContextList> = await apiClient(
        showServerDownPopup,
      )
        .node()
        .getContexts();
        console.log(fetchContextsResponse.data);
    }
    fetchAvailableContexts();
  }, []);
  return (
    <Modal
      show={showPopup}
      backdrop="static"
      keyboard={false}
      aria-labelledby="contained-modal-title-vcenter"
      centered
    >
      <ModalWrapper>
        <div className="title">Sign-in request</div>
        <div className="subtitle">
          This site: {' '}
          <a href={callbackUrl} target="_blank" rel="noreferrer" className='app-callbackurl'>
            {callbackUrl}
          </a>, running application:{' '}
          <span className="app-id">{applicationId}</span> requested to sign in
        </div>
        <div className="context-list">
          <div className="context-title">Available contexts</div>
        </div>
      </ModalWrapper>
    </Modal>
  );
}
