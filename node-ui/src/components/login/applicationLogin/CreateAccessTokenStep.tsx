import React from 'react';
import Button from '../../common/Button';
import { ModalWrapper } from './SelectContextStep';
import translations from '../../../constants/en.global.json';

interface CreateAccessTokenStepProps {
  applicationId: string;
  callbackUrl: string;
  selectedContextId: string;
  selectedIdentity: string;
  onCreateToken: () => void;
  errorMessage: string;
}

export default function CreateAccessTokenStep({
  applicationId,
  callbackUrl,
  selectedContextId,
  selectedIdentity,
  onCreateToken,
  errorMessage,
}: CreateAccessTokenStepProps) {
  const t = translations.appLoginPopup.createToken;
  return (
    <ModalWrapper>
      <div className="title">{t.title}</div>
      <div className="wrapper">
        <div className="subtitle">
          {t.websiteText}
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
          {t.appIdText}
          <span className="app-id">{applicationId}</span>
        </div>
        <div className="subtitle">
          {t.contextIdText}
          <span className="app-id">{selectedContextId}</span>
        </div>
        <div className="subtitle">
          {t.contextIdentityText}
          <span className="app-id">{selectedIdentity}</span>
        </div>
      </div>
      <div className="wrapper">
        <Button text={t.buttonNextText} onClick={onCreateToken} width="100%" />
      </div>
      <div className="no-context-text">
        <span className="app-id error">{errorMessage}</span>
      </div>
    </ModalWrapper>
  );
}
