import React from 'react';
import { styled } from 'styled-components';
import Button from '../../common/Button';
import ListItem from './ListItem';
import translations from '../../../constants/en.global.json';

export const ModalWrapper = styled.div`
  display: flex;
  flex-direction: column;
  justify-content: center;
  padding: 1.5rem;
  border-radius: 0.375rem;
  items-align: center;
  background-color: #17191b;

  .title,
  .context-title {
    font-size: 1rem;
    font-weight: 700;
    line-height: 1.25rem;
    color: #fff;
  }
  .title {
    text-align: center;
  }

  .subtitle,
  .context-subtitle {
    color: #6b7280;
    font-weigth: 500;
    font-size: 0.875rem;
  }

  .subtitle {
    word-break: break-all;
  }

  .wrapper {
    margin-top: 1.25rem;
    color: #fff;
  }

  .app-id {
    color: #6b7280;
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

  .list-wrapper {
    background-color: #17191b;
  }

  .flex-container {
    margin-top: 1rem;
    display: flex;
    flex-direction: column;
    gap: 1rem;
  }

  .selected-text-wrapper {
    display: flex;
    flex-direction: column;
  }
  .no-context-text {
    text-align: center;
  }
`;

interface SelectIdentityStepProps {
  applicationId: string;
  callbackUrl: string;
  contextIdentities: string[];
  selectedIdentity: string;
  setSelectedIdentity: (selectedIdentity: string) => void;
  updateLoginStep: () => void;
  finishLogin: () => void;
}

export default function SelectIdentityStep({
  applicationId,
  callbackUrl,
  contextIdentities,
  selectedIdentity,
  setSelectedIdentity,
  updateLoginStep,
  finishLogin,
}: SelectIdentityStepProps) {
  const t = translations.appLoginPopup.selectIdentity;
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
          <span className="app-id">{applicationId}</span>
        </div>
      </div>
      <div className="wrapper">
        <div className="context-title">{t.contextsTitle}</div>
        <div className="context-subtitle">{t.contextsSubtitle}</div>
        <div className="context-list">
          {contextIdentities.length > 0 ? (
            contextIdentities.map((identity, i) => (
              <ListItem
                item={identity}
                id={i}
                count={contextIdentities.length}
                onRowItemClick={setSelectedIdentity}
                key={i}
              />
            ))
          ) : (
            <div className="flex-container">
              <div className="no-context-text">{t.noContextsText}</div>
              <Button
                text={t.buttonBackText}
                onClick={finishLogin}
                width="100%"
              />
            </div>
          )}
        </div>
      </div>
      <div>
        {selectedIdentity && (
          <div className="flex-container">
            <div className="selected-text-wrapper">
              <span className="subtitle">{t.selectedContextText}</span>
              <span className="context-title">{selectedIdentity}</span>
            </div>
            <Button
              text={t.buttonNextText}
              onClick={updateLoginStep}
              width="100%"
            />
          </div>
        )}
      </div>
    </ModalWrapper>
  );
}
