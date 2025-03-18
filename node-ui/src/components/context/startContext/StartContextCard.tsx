import React from 'react';
import styled from 'styled-components';
import Button from '../../common/Button';
import ApplicationsPopup from './ApplicationsPopup';
import translations from '../../../constants/en.global.json';
import StatusModal, { ModalContent } from '../../common/StatusModal';
import { ContextApplication } from '../../../pages/StartContext';
import { XMarkIcon } from '@heroicons/react/24/solid';

const Wrapper = styled.div`
  display: flex;
  flex: 1;
  flex-direction: column;
  padding: 1rem;
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

  .cancel-icon {
    position: relative;
    right: -0.25rem;
    cursor: pointer;
    height: 1.25rem;
    width: 1.25rem;
    color: #fff;
    cursor: pointer;
    &:hover {
      color: #4cfafc;
    }
  }

  .select-app-section {
    .button-container {
      display: flex;
      padding-top: 1rem;
      gap: 1rem;
    }

    .selected-app {
      display: flex;
      flex-direction: column;
      padding-top: 0.25rem;
      padding-left: 0.5rem;
      font-size: 0.875rem;
      font-weight: 500;
      line-height: 1.25rem;
      text-align: left;
      .label {
        color: #6b7280;
      }
      .value {
        color: #fff;
      }
    }
  }

  .init-section {
    padding-top: 1rem;
    display: flex;
    flex-direction: column;
    gap: 1rem;

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
        height: 12.5rem;
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

interface StartContextCardProps {
  application: ContextApplication;
  setApplication: (application: ContextApplication) => void;
  isArgsChecked: boolean;
  setIsArgsChecked: (checked: boolean) => void;
  argumentsJson: string;
  setArgumentsJson: (args: string) => void;
  startContext: () => void;
  setProtocol: (protocol: string) => void;
  showBrowseApplication: boolean;
  setShowBrowseApplication: (show: boolean) => void;
  onUploadClick: () => void;
  isLoading: boolean;
  showStatusModal: boolean;
  closeModal: () => void;
  startContextStatus: ModalContent;
}

export default function StartContextCard({
  application,
  setApplication,
  isArgsChecked,
  setIsArgsChecked,
  argumentsJson,
  setArgumentsJson,
  startContext,
  showBrowseApplication,
  setProtocol,
  setShowBrowseApplication,
  onUploadClick,
  isLoading,
  showStatusModal,
  closeModal,
  startContextStatus,
}: StartContextCardProps) {
  const t = translations.startContextPage;
  const onStartContextClick = async () => {
    if (!application.appId) {
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
      {showBrowseApplication && (
        <ApplicationsPopup
          show={showBrowseApplication}
          closeModal={() => setShowBrowseApplication(false)}
          setApplication={setApplication}
        />
      )}
      <div className="select-app-section">
        <div className="section-title">
          {application.appId
            ? t.selectedApplicationTitle
            : t.selectApplicationTitle}
          {application.appId && (
            <XMarkIcon
              className="cancel-icon"
              onClick={() =>
                setApplication({
                  appId: '',
                  name: '',
                  version: '',
                  path: '',
                  hash: '',
                })
              }
            />
          )}
        </div>
        {application.appId ? (
          <div className="selected-app">
            <p className="label">
              {t.idLabelText}
              <span className="value">{application.appId}</span>
            </p>
            <p className="label">
              {t.nameLabelText}
              <span className="value">{application.name}</span>
            </p>
            <p className="label">
              {t.versionLabelText}
              <span className="value">{application.version}</span>
            </p>
          </div>
        ) : (
          <div className="button-container">
            <Button
              text="Browse"
              width={'144px'}
              onClick={() => setShowBrowseApplication(true)}
            />
            <Button text="Upload" width={'144px'} onClick={onUploadClick} />
          </div>
        )}
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
            <div className="section-title">{t.argsTitleText}</div>
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
            <option value="ethereum">Ethereum</option>
          </select>
        </div>
        <Button
          text="Start"
          width={'144px'}
          onClick={onStartContextClick}
          isLoading={isLoading}
        />
      </div>
    </Wrapper>
  );
}
