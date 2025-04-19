import React from 'react';
import { styled } from 'styled-components';
import ListItem from './ListItem';
import translations from '../../../constants/en.global.json';
import { truncateText } from '../../../utils/displayFunctions';
import { Tooltip } from 'react-tooltip';
import { ClipboardDocumentIcon, XCircleIcon } from '@heroicons/react/24/solid';
import { copyToClipboard } from '../../../utils/copyToClipboard';

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

  .context-title {
    color: #fff;
    font-size: 1rem;
    font-weight: 500;
  }

  .subtitle,
  .context-subtitle {
    color: #6b7280;
    font-weigth: 500;
    font-size: 0.875rem;
  }

  .subtitle {
    word-break: break-all;
    display: flex;
    gap: 0.5rem;
  }

  .wrapper {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    margin-top: 1.25rem;
    color: #fff;
  }

  .label {
    color: #fff;
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }

  .app-callbackurl {
    color: #6b7280;
    text-decoration: none;
  }

  .context-list {
    display: flex;
    flex-direction: column;
    max-height: 200px;
    overflow-y: scroll;
  }

  .list-item {
    padding-top: 0.25rem;
    padding-bottom: 0.25rem;
    width: fit-content;
    white-space: break-spaces;
    margin-top: 1.25rem;
    color: #fff;
    font-weigth: 500;
    font-size: 0.875rem;
    cursor: pointer;
  }

  .list-item:hover {
    color: #4cfafc;
  }

  .flex-container {
    margin-top: 1rem;
    display: flex;
    flex-direction: column;
    gap: 1rem;
  }

  .no-context-text {
    text-align: center;
    font-size: 0.875rem;
  }

  .back-button {
    margin-top: 1rem;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 0.5rem;
    color: #fff;
    cursor: pointer;
  }

  .back-text {
    color: #6b7280;
    font-size: 0.875rem;
  }
  .back-text:hover {
    color: #fff;
  }

  .copy-icon {
    height: 1rem;
    width: 1rem;
    color: #fff;
    cursor: pointer;
  }
  .copy-icon:hover {
    color: #9c9da3;
  }

  .separator {
    border-bottom: 1px solid #23262d;
  }

  .flex {
    display: flex;
    justify-content: space-between;
  }

  .step {
    color: #fff;
    font-size: 0.875rem;
    position: absolute;
    top: 1rem;
  }
  .close {
    position: absolute;
    top: 1rem;
    right: 1rem;
    cursor: pointer;
  }
`;

interface SelectIdentityStepProps {
  applicationId: string;
  callbackUrl: string;
  contextIdentities: string[];
  selectedContextId: string;
  updateLoginStep: (selectedIdentity: string) => void;
  backLoginStep: () => void;
  closePopup: () => void;
}

export default function SelectIdentityStep({
  applicationId,
  callbackUrl,
  contextIdentities,
  selectedContextId,
  updateLoginStep,
  backLoginStep,
  closePopup,
}: SelectIdentityStepProps) {
  const t = translations.appLoginPopup.selectIdentity;

  return (
    <ModalWrapper>
      <span className="step">2/3</span>
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
      </div>
      <div className="wrapper">
        <div>
          <div className="context-title">{t.contextsTitle}</div>
          <div className="context-subtitle">{t.contextsSubtitle}</div>
        </div>
        <div className="context-list">
          {contextIdentities.length > 0 ? (
            contextIdentities.map((identity, i) => (
              <ListItem
                item={identity}
                id={i}
                count={contextIdentities.length}
                onRowItemClick={updateLoginStep}
                key={i}
              />
            ))
          ) : (
            <div className="flex-container">
              <div className="no-context-text">{t.noContextsText}</div>
            </div>
          )}
        </div>
      </div>
      <div className="flex-center">
        <div className="back-button" onClick={backLoginStep}>
          <span className="back-text">{t.backButtonText}</span>
        </div>
      </div>
    </ModalWrapper>
  );
}
