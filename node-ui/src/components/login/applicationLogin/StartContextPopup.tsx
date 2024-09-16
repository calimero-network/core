import React from 'react';
import styled from 'styled-components';
import Button from '../../common/Button';
import translations from '../../../constants/en.global.json';
import StatusModal, { ModalContent } from '../../common/StatusModal';
import { DisplayApplication } from './StartContextStep';

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

  .flex-wrapper-buttons {
    display: flex;
    gap: 1rem;
    width: 100%;
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
  navigateBack: () => void;
}

export default function StartContextPopup({
  application,
  isArgsChecked,
  setIsArgsChecked,
  argumentsJson,
  setArgumentsJson,
  startContext,
  isLoading,
  showStatusModal,
  closeModal,
  startContextStatus,
  navigateBack,
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
      <div className="select-app-section">
        <div className="selected-app">
          <p className="label">
            {t.idLabelText}
            <span className="value">{application?.appId}</span>
          </p>
          <p className="label">
            {t.nameLabelText}
            <span className="value">{application?.name}</span>
          </p>
          <p className="label">
            {t.versionLabelText}
            <span className="value">{application?.version}</span>
          </p>
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
        <div className="flex-wrapper-buttons">
          <Button
            text="Back to context list"
            width="100%"
            onClick={navigateBack}
            isDisabled={isLoading}
            color="#6b7280"
            highlightColor='#6b7280'
            disabledColor='#6b7280'
          />
          <Button
            text="Start"
            width="100%"
            onClick={onStartContextClick}
            isLoading={isLoading}
            isDisabled={isLoading}
          />
        </div>
      </div>
    </Wrapper>
  );
}
