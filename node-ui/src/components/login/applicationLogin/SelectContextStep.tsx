import React from 'react';
import { styled } from 'styled-components';
import Button from '../../common/Button';
import { Context } from '../../../api/dataSource/NodeDataSource';
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

  .wrap-text {
    word-wrap: break-word;
    overflow-wrap: break-word;
  }

  .no-context-text {
    text-align: center;
  }

  .error {
    color: #ef4444;
    font-size: 0.875rem;
  }
`;

interface SelectContextStepProps {
  applicationId: string;
  callbackUrl: string;
  contextList: Context[];
  selectedContextId: string;
  setSelectedContextId: (selectedContextId: string) => void;
  updateLoginStep: () => void;
  finishLogin: () => void;
  createContext: () => void;
}

export default function SelectContextStep({
  applicationId,
  callbackUrl,
  contextList,
  selectedContextId,
  setSelectedContextId,
  updateLoginStep,
  finishLogin,
  createContext,
}: SelectContextStepProps) {
  const t = translations.appLoginPopup.selectContext;
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
      </div>
      <div className="wrapper">
        <div className="context-title">{t.contextsTitle}</div>
        <div className="context-subtitle">{t.contextsSubtitle}</div>
        {contextList.length > 0 ? (
          <>
            <div className="context-list">
              {contextList.map((context, i) => (
                <ListItem
                  item={context.id}
                  id={i}
                  count={contextList.length}
                  onRowItemClick={setSelectedContextId}
                  key={i}
                />
              ))}
            </div>
            <Button
              text={t.buttonCreateText}
              onClick={createContext}
              width="100%"
            />
          </>
        ) : (
          <div className="flex-container">
            <div className="no-context-text">{t.noContextsText}</div>
            <Button
              text={t.buttonCreateText}
              onClick={createContext}
              width="100%"
            />
            <Button
              text={t.buttonBackText}
              onClick={finishLogin}
              width="100%"
            />
          </div>
        )}
      </div>
      <div>
        {selectedContextId && (
          <div className="flex-container">
            <div className="selected-text-wrapper">
              <span className="subtitle wrap-text">
                {t.selectedContextText}
              </span>
              <span className="context-title wrap-text">
                {selectedContextId}
              </span>
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
