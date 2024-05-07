import React from "react";
import styled from "styled-components";
import PropTypes from "prop-types";
import Button from "../../common/Button";
import ApplicationsPopup from "./ApplicationsPopup";
import translations from "../../../constants/en.global.json";

const Wrapper = styled.div`
  display: flex;
  flex: 1;
  flex-direction: column;
  padding: 1rem;

  .section-title {
    font-family: Inter;
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
      font-family: Inter;
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
        font-family: Inter;
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
        height: 200px;
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

export default function StartContextCard({
  application,
  setApplication,
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
}) {
  const t = translations.startContextPage;
  const onStartContextClick = async () => {
    if (!application) {
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
      {showBrowseApplication && (
        <ApplicationsPopup
          show={showBrowseApplication}
          closeModal={() => setShowBrowseApplication(false)}
          setApplication={setApplication}
        />
      )}
      <div className="select-app-section">
        <div className="section-title">
          {application ? t.selectedApplicationTitle : t.selectApplicationTitle}
        </div>
        {application ? (
          <div className="selected-app" onClick={() => setApplication(null)}>
            {application.name}
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

StartContextCard.propTypes = {
  application: PropTypes.object,
  setApplication: PropTypes.func,
  isArgsChecked: PropTypes.bool,
  setIsArgsChecked: PropTypes.func,
  methodName: PropTypes.string,
  setMethodName: PropTypes.func,
  argumentsJson: PropTypes.string,
  setArgumentsJson: PropTypes.func,
  startContext: PropTypes.func,
  showBrowseApplication: PropTypes.bool.isRequired,
  setShowBrowseApplication: PropTypes.func.isRequired,
  onUploadClick: PropTypes.func.isRequired,
  isLoading: PropTypes.bool.isRequired,
};
