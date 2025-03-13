import React from 'react';
import styled from 'styled-components';
import Button from '../../common/Button';
import translations from '../../../constants/en.global.json';
import StatusModal, { ModalContent } from '../../common/StatusModal';
import { DisplayApplication } from './StartContextStep';
import { truncateText } from '../../../utils/displayFunctions';
import { Tooltip } from 'react-tooltip';
import { ClipboardDocumentIcon } from '@heroicons/react/24/solid';
import { copyToClipboard } from '../../../utils/copyToClipboard';

const Wrapper = styled.div`
  display: flex;
  flex: 1;
  flex-direction: column;
  font-optical-sizing: auto;
  font-weight: 500;
  font-style: normal;
  font-variation-settings: 'slnt' 0;
  -webkit-font-smoothing: antialiased;
  -moz-osx-font-smoothing: grayscale;
  font-smooth: never;

  .section-title {
    font-size: 0.875rem;
    font-weight: 500;
    line-height: 1.25rem;
    text-align: left;
    color: #6b7280;
  }

  .subtitle {
    color: #6b7280;
    font-weigth: 500;
    font-size: 0.875rem;
    word-break: break-all;
    display: flex;
    gap: 0.5rem;
  }

  .separator {
    border-bottom: 1px solid #23262d;
  }

  .app-info-wrapper {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    margin-top: 1.25rem;
    color: #fff;
  }

  .info-item {
    display: flex;
    gap: 0.5rem;
    color: #6b7280;
    font-weigth: 500;
    font-size: 0.875rem;
  }

  .label-id {
    color: #fff;
    display: flex;
    align-items: center;
    word-break: break-all;
    gap: 0.5rem;
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

  .back-button {
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

  .flex-wrapper-buttons {
    display: flex;
    gap: 1rem;
    width: 100%;
  }

  .init-section {
    padding-top: 1rem;
    display: flex;
    flex-direction: column;
    gap: 0.5rem;

    .init-title {
      display: flex;
      justify-content: flex-start;
      align-items: center;
      gap: 0.5rem;
    }

    .form-check-input {
      margin: 0;
      padding: 0;
      background-color: #121216;
      border: 1px solid #4cfafc;
    }

    .input {
      margin-top: 1rem;
      display: flex;
      flex-direction: column;
      gap: 0.5rem;

      .label {
        display: flex;
        gap: 0.5rem;
        font-size: 0.75rem;
        font-weight: 500;
        line-height: 0.875rem;
        text-align: left;
        color: #6b7280;
      }

      .method-input {
        width: 30%;
        font-size: 0.875rem;
        font-weight: 500;
        line-height: 0.875rem;
        padding: 0.25rem;
      }

      .args-input {
        position: relative;
        height: 5rem;
        font-size: 0.875rem;
        font-weight: 500;
        line-height: 0.875rem;
        padding: 0.25rem;
        resize: none;
      }

      .flex-wrapper {
        display: flex;
        justify-content: flex-end;
        padding-right: 0.5rem;
      }

      .format-btn {
        cursor: pointer;
        font-size: 0.825rem;
        font-weight: 500;
        line-height: 0.875rem;

        &:hover {
          color: #4cfafc;
        }
      }
    }
  }

  .protocol-input {
    width: 100%;
    padding: 8px;
    border: 1px solid #ccc;
    border-radius: 4px;
    font-size: 14px;
    background-color: white;
  }

  .protocol-input:focus {
    outline: none;
    border-color: #007bff;
  }

  /* Optional: Style the dropdown arrow */
  .protocol-input {
    appearance: none;
    background-image: url('data:image/svg+xml;charset=US-ASCII,%3Csvg%20xmlns%3D%22http%3A%2F%2Fwww.w3.org%2F2000%2Fsvg%22%20width%3D%22292.4%22%20height%3D%22292.4%22%3E%3Cpath%20fill%3D%22%23007CB2%22%20d%3D%22M287%2069.4a17.6%2017.6%200%200%200-13-5.4H18.4c-5%200-9.3%201.8-12.9%205.4A17.6%2017.6%200%200%200%200%2082.2c0%205%201.8%209.3%205.4%2012.9l128%20127.9c3.6%203.6%207.8%205.4%2012.8%205.4s9.2-1.8%2012.8-5.4L287%2095c3.5-3.5%205.4-7.8%205.4-12.8%200-5-1.9-9.2-5.4-12.8z%22%2F%3E%3C%2Fsvg%3E');
    background-repeat: no-repeat;
    background-position: right 8px center;
    background-size: 12px;
    padding-right: 30px;
    background-color: transparent;
    margin-top: 5px;
  }
`;

interface StartContextPopupProps {
  application: DisplayApplication | null;
  isArgsChecked: boolean;
  setIsArgsChecked: (checked: boolean) => void;
  argumentsJson: string;
  setArgumentsJson: (args: string) => void;
  startContext: () => void;
  isLoading: boolean;
  showStatusModal: boolean;
  closeModal: () => void;
  startContextStatus: ModalContent;
  backLoginStep: () => void;
  setProtocol: (protocol: string) => void;
}

export default function StartContextPopup({
  application,
  isArgsChecked,
  setIsArgsChecked,
  argumentsJson,
  setArgumentsJson,
  startContext,
  setProtocol,
  isLoading,
  showStatusModal,
  closeModal,
  startContextStatus,
  backLoginStep,
}: StartContextPopupProps) {
  const t = translations.startContextPage;
  const onStartContextClick = async () => {
    if (!application?.appId) {
      return;
    } else if (isArgsChecked && !argumentsJson) {
      return;
    } else {
      await startContext();
    }
  };

  const formatArguments = () => {
    try {
      const formattedJson = JSON.stringify(JSON.parse(argumentsJson), null, 2);
      setArgumentsJson(formattedJson);
    } catch (error) {
      console.log('error', error);
    }
  };

  return (
    <Wrapper>
      <StatusModal
        show={showStatusModal}
        closeModal={closeModal}
        modalContent={startContextStatus}
      />
      <div className="app-info-wrapper">
        <div className="subtitle separator">
          <span>{t.detailsText}</span>
        </div>
        <div className="info-item">
          <span>{t.idLabelText}</span>
          <div className="label-id" data-tooltip-id="tooltip">
            <span>{truncateText(application?.appId ?? '-')}</span>
            <Tooltip id="tooltip" content={application?.appId ?? '-'} />
            <ClipboardDocumentIcon
              className="copy-icon"
              onClick={() => copyToClipboard(application?.appId ?? '-')}
            />
          </div>
        </div>
        <div className="info-item">
          <span>{t.nameLabelText}</span>
          <span className="label-id">{application?.name ?? '-'}</span>
        </div>
        <div className="info-item">
          <span>{t.versionLabelText}</span>
          <span className="label-id">{application?.version ?? '-'}</span>
        </div>
      </div>
      <div className="init-section">
        <div className="init-title">
          <input
            className="form-check-input"
            type="checkbox"
            value=""
            id="flexCheckChecked"
            checked={isArgsChecked}
            onChange={() => setIsArgsChecked(!isArgsChecked)}
          />
          <div className="section-title">{t.initSectionTitle}</div>
        </div>
        {isArgsChecked && (
          <div className="args-section">
            <div className="input">
              <label className="label">{t.argsLabelText}</label>
              <textarea
                className="args-input"
                value={argumentsJson}
                onChange={(e) => setArgumentsJson(e.target.value)}
              />
              <div className="flex-wrapper">
                <div className="format-btn" onClick={formatArguments}>
                  {t.buttonFormatText}
                </div>
              </div>
            </div>
          </div>
        )}
        <div className="protocol-section">
          <div className="protocol-title">{t.protocolLabelText}</div>
          <select
            className="protocol-input"
            onChange={(e) => setProtocol(e.target.value)}
            defaultValue=""
            required
          >
            <option value="" disabled>
              Select a protocol
            </option>
            <option value="near">NEAR</option>
            <option value="starknet">Starknet</option>
            <option value="icp">ICP</option>
            <option value="stellar">Stellar</option>
            <option value="evm">EVM</option>
          </select>
        </div>
        <div className="flex-wrapper-buttons">
          <Button
            text="Start"
            width="100%"
            onClick={onStartContextClick}
            isLoading={isLoading}
            isDisabled={isLoading}
          />
        </div>
        <div className="flex-center">
          <div className="back-button" onClick={backLoginStep}>
            <span className="back-text">{t.backButtonText}</span>
          </div>
        </div>
      </div>
    </Wrapper>
  );
}
