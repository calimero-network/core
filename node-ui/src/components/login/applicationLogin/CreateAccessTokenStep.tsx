import React from 'react';
import Button from '../../common/Button';
import { ModalWrapper } from './SelectContextStep';

interface CreateAccessTokenStepProps {
  applicationId: string;
  callbackUrl: string;
  selectedContextId: string;
  onCreateToken: () => void;
}

export default function CreateAccessTokenStep({
  applicationId,
  callbackUrl,
  selectedContextId,
  onCreateToken,
}: CreateAccessTokenStepProps) {
  return (
    <ModalWrapper>
      <div className="title">Sign-in request</div>
      <div className="wrapper">
        <div className="subtitle">
          From site:{' '}
          <a
            href={callbackUrl}
            target="_blank"
            rel="noreferrer"
            className="app-callbackurl"
          >
            {callbackUrl}
          </a>
        </div>
        <div className="subtitle">
          Application Id: <span className="app-id">{applicationId}</span>
        </div>
        <div className="subtitle">
          Context Id: <span className="app-id">{selectedContextId}</span>
        </div>
      </div>
      <div className="wrapper">
        <Button text="Create token" onClick={onCreateToken} width="100%" />
      </div>
    </ModalWrapper>
  );
}
