import React from 'react';
import { styled } from 'styled-components';
import Button from '../../common/Button';
import { Context } from '../../../api/dataSource/NodeDataSource';
import ListItem from './ListItem';
import translations from '../../../constants/en.global.json';
import { truncateText } from '../../../utils/displayFunctions';
import { ClipboardDocumentIcon } from '@heroicons/react/24/solid';
import { Tooltip } from 'react-tooltip';

export const ModalWrapper = styled.div`
  display: flex;
  position: relative;
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

  .app-id {
    color: #fff;
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }

  .app-callbackurl {
    color: #fff;
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

  .error {
    color: #ef4444;
    font-size: 0.875rem;
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
`;

interface SelectContextStepProps {
  applicationId: string;
  callbackUrl: string;
  contextList: Context[];
  setSelectedContextId: (selectedContextId: string) => void;
  updateLoginStep: () => void;
  createContext: () => void;
}

export default function SelectContextStep({
  applicationId,
  callbackUrl,
  contextList,
  setSelectedContextId,
  updateLoginStep,
  createContext,
}: SelectContextStepProps) {
  const t = translations.appLoginPopup.selectContext;

  const copyToClippboard = (text: string) => {
    navigator.clipboard.writeText(text).catch((err) => {
      console.error('Failed to copy text to clipboard: ', err);
    });
  };

  return (
    <ModalWrapper>
      <div className="step">1/3</div>
      <div className="title">{t.title}</div>
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
          <div className="app-id" data-tooltip-id="tooltip">
            <span>{truncateText(applicationId)}</span>
            <Tooltip id="tooltip" content={applicationId} />
            <ClipboardDocumentIcon
              className="copy-icon"
              onClick={() => copyToClippboard(applicationId)}
            />
          </div>
        </div>
      </div>
      <div className="wrapper">
        <div className="flex">
          <div>
            <div className="context-title">{t.contextsTitle}</div>
            <div className="context-subtitle">{t.contextsSubtitle}</div>
          </div>
          <Button
            text={t.buttonCreateText}
            onClick={createContext}
            width="200px"
          />
        </div>

        {contextList.length > 0 ? (
          <>
            <div className="context-list">
              {contextList.map((context, i) => (
                <ListItem
                  item={context.id}
                  id={i}
                  count={contextList.length}
                  onRowItemClick={(selectedContextId: string) => {
                    setSelectedContextId(selectedContextId);
                    updateLoginStep();
                  }}
                  key={i}
                />
              ))}
            </div>
          </>
        ) : (
          <div className="flex-container">
            <div className="no-context-text">{t.noContextsText}</div>
          </div>
        )}
      </div>
    </ModalWrapper>
  );
}
