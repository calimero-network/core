import React from 'react';
import Button from '../../common/Button';
import translations from '../../../constants/en.global.json';
import { ModalWrapper } from './SelectIdentityStep';
import { truncateText } from '../../../utils/displayFunctions';
import { Tooltip } from 'react-tooltip';
import { ClipboardDocumentIcon, XCircleIcon } from '@heroicons/react/24/solid';
import { copyToClipboard } from '../../../utils/copyToClipboard';

interface CreateAccessTokenStepProps {
  applicationId: string;
  callbackUrl: string;
  selectedContextId: string;
  selectedIdentity: string;
  onCreateToken: () => void;
  errorMessage: string;
  backLoginStep: () => void;
  closePopup: () => void;
}

export default function CreateAccessTokenStep({
  applicationId,
  callbackUrl,
  selectedContextId,
  selectedIdentity,
  onCreateToken,
  errorMessage,
  backLoginStep,
  closePopup,
}: CreateAccessTokenStepProps) {
  const t = translations.appLoginPopup.createToken;

  return (
    <ModalWrapper>
      <div className="step">3/3</div>
      <div className="title">{t.title}</div>
      <div className="close">
        <XCircleIcon className="copy-icon" onClick={closePopup} />
      </div>
      <div className="wrapper">
        <div className="subtitle separator">
          <span>{t.detailsText}</span>
        </div>
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
          <div className="label" data-tooltip-id="tooltip">
            <span>{truncateText(applicationId)}</span>
            <Tooltip id="tooltip" content={applicationId} />
            <ClipboardDocumentIcon
              className="copy-icon"
              onClick={() => copyToClipboard(applicationId)}
            />
          </div>
        </div>
        <div className="subtitle">
          {t.contextIdText}
          <div className="label" data-tooltip-id="tooltip">
            <span>{truncateText(selectedContextId)}</span>
            <Tooltip id="tooltip" content={selectedContextId} />
            <ClipboardDocumentIcon
              className="copy-icon"
              onClick={() => copyToClipboard(selectedContextId)}
            />
          </div>
        </div>
        <div className="subtitle">
          {t.contextIdentityText}
          <div className="label" data-tooltip-id="tooltip">
            <span>{truncateText(selectedIdentity)}</span>
            <Tooltip id="tooltip" content={selectedIdentity} />
            <ClipboardDocumentIcon
              className="copy-icon"
              onClick={() => copyToClipboard(selectedIdentity)}
            />
          </div>
        </div>
      </div>

      <div className="wrapper">
        <Button text={t.buttonNextText} onClick={onCreateToken} width="100%" />
      </div>
      <div className="no-context-text">
        <span className="app-id error">{errorMessage}</span>
      </div>
      <div className="flex-center">
        <div className="back-button" onClick={backLoginStep}>
          <span className="back-text">{t.backButtonText}</span>
        </div>
      </div>
    </ModalWrapper>
  );
}
