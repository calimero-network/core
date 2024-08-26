import React from 'react';
import { styled } from 'styled-components';
import Button from '../../common/Button';
import ContextListItem from './ContextListItem';
import { Context } from '../../../api/dataSource/NodeDataSource';

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
`;

interface SelectContextStepProps {
  applicationId: string;
  callbackUrl: string;
  contextList: Context[];
  selectedContextId: string;
  setSelectedContextId: (selectedContextId: string) => void;
  updateLoginStep: () => void;
}

export default function SelectContextStep({
  applicationId,
  callbackUrl,
  contextList,
  selectedContextId,
  setSelectedContextId,
  updateLoginStep,
}: SelectContextStepProps) {
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
      </div>
      <div className="wrapper">
        <div className="context-title">Available contexts</div>
        <div className="context-subtitle">
          Select context to create login token
        </div>
        <div className="context-list">
          {contextList.map((context, i) => (
            <ContextListItem
              item={context}
              id={i}
              count={contextList.length}
              onRowItemClick={setSelectedContextId}
            />
          ))}
        </div>
      </div>
      <div>
        {selectedContextId && (
          <div className="flex-container">
            <div className='selected-text-wrapper'>
              <span className="subtitle">Selected context ID:</span>
              <span className="context-title">{selectedContextId}</span>
            </div>
            <Button
              text="Confirm context id"
              onClick={updateLoginStep}
              width="100%"
            />
          </div>
        )}
      </div>
    </ModalWrapper>
  );
}
