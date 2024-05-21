import React from "react";
import styled from "styled-components";
import Button from "../../common/Button";
import ApplicationsPopup from "./ApplicationsPopup";
import translations from "../../../constants/en.global.json";
import StatusModal, { ModalContent } from "../../common/StatusModal";

const Wrapper = styled.div`
  display: flex;
  flex: 1;
  flex-direction: column;
  padding: 1rem;
  font-optical-sizing: auto;
  font-weight: 500;
  font-style: normal;
  font-variation-settings: "slnt" 0;
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

  .select-app-section {
    .button-container {
      display: flex;
      padding-top: 1rem;
      gap: 1rem;
    }

    .selected-app {
      padding-top: 0.25rem;
      padding-left: 0.5rem;
      font-size: 0.875rem;
      font-weight: 500;
      line-height: 1.25rem;
      text-align: left;
      color: #fff;
      cursor: pointer;

      &:hover {
        color: #4cfafc;
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
`;

interface StartContextCardProps {
  applicationId: string;
  setApplicationId: (applicationId: string) => void;
  isArgsChecked: boolean;
  setIsArgsChecked: (checked: boolean) => void;
  methodName: string;
  setMethodName: (method: string) => void;
  argumentsJson: string;
  setArgumentsJson: (args: string) => void;
  startContext: () => void;
  showBrowseApplication: boolean;
  setShowBrowseApplication: (show: boolean) => void;
  onUploadClick: () => void;
  isLoading: boolean;
  showStatusModal: boolean;
  closeModal: () => void;
  startContextStatus: ModalContent;
}

export default function StartContextCard({
  applicationId,
  setApplicationId,
  isArgsChecked,
  setIsArgsChecked,
  methodName,
  setMethodName,
  argumentsJson,
  setArgumentsJson,
  startContext,
  showBrowseApplication,
  setShowBrowseApplication,
  onUploadClick,
  isLoading,
  showStatusModal,
  closeModal,
  startContextStatus
}: StartContextCardProps) {
  const t = translations.startContextPage;
  const onStartContextClick = async () => {
    if (!applicationId) {
      return;
    } else if (isArgsChecked && (!methodName || !argumentsJson)) {
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
      console.log("error", error);
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
          setApplicationId={setApplicationId}
        />
      )}
      <div className="select-app-section">
        <div className="section-title">
          {applicationId ? t.selectedApplicationTitle : t.selectApplicationTitle}
        </div>
        {applicationId ? (
          <div className="selected-app" onClick={() => setApplicationId("")}>
            {applicationId}
          </div>
        ) : (
          <div className="button-container">
            <Button
              text="Browse"
              width={"144px"}
              onClick={() => setShowBrowseApplication(true)}
            />
            <Button text="Upload" width={"144px"} onClick={onUploadClick} />
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
              <label className="label">{t.methodLabelText}</label>
              <input
                className="method-input"
                value={methodName}
                onChange={(e) => setMethodName(e.target.value)}
              />
            </div>
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
        <Button text="Start" width={"144px"} onClick={onStartContextClick} isLoading={isLoading}/>
      </div>
    </Wrapper>
  );
}
