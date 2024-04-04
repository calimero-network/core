import React from "react";
import styled from "styled-components";
import PropTypes from "prop-types";
import translations from "../../constants/en.global.json";

const Wrapper = styled.div`
  .upload-form {
    padding: 12px;
    display: flex;
    flex-direction: column;

    .title {
      font-size: 14px;
      color: #fff;
      margin-bottom: 12px;
    }

    .label {
      font-size: 12px;
      color: rgb(255, 255, 255, 0.7);
    }
    .file-selection {
      margin-top: 8px;
      margin-bottom: 12px;
      color: #fff;
      display: flex;
      gap: 12px;
      width: fit-content;
    }

    .upload-button, .download-button {
      border-radius: 4px;
      background-color: rgba(255, 255, 255, 0.06);
      width: fit-content;
      height: 30px;
      width: 97.28px;
      padding-left: 14px;
      padding-right: 14px;
      margin-top: 8px;
      cursor: pointer;
      border: none;
      outline: none;
    }
    .upload-button:hover, .download-button:hover {
      background-color: rgba(255, 255, 255, 0.12);
    }
    .download-button {
      margin-top: 8px;
      width: fit-content;
    }
    .file-details {
      margin-top: 24px;
      background-color: #17171d;
      display: flex;
      flex-direction: column;
      gap: 4px;
      border-radius: 4px;
      padding: 8px;
      overflow: hidden;
      white-space: nowrap;
      text-overflow: ellipsis;
    }
    .text {
      color: #fff;
      font-size: 14px;
    }
    .download-url {
      margin-top: 8px;
      text-decoration: none;
      color: #ff842d;
      font-size: 14px;
    }
`;

export function UploadApplication({
  handleFileChange,
  handleFileUpload,
  createDownloadUrl,
  cidString,
  wasmFile,
  downloadUrl,
}) {
  const t = translations.uploadApplication;

  return (
    <Wrapper>
      <div className="upload-form">
        <div className="title">{t.title}</div>
        <label className="label">{t.inputLabelText}</label>
        <input
          className="file-selection"
          type="file"
          accept=".wasm"
          onChange={handleFileChange}
        />
        <label className="label">{t.buttonUploadLabel}</label>
        <button
          className="upload-button"
          onClick={handleFileUpload}
          disabled={!wasmFile}
        >
          {t.buttonUploadText}
        </button>
        {cidString && (
          <div className="file-details">
            <label className="label">{t.cidLabelText}</label>
            <div className="text">{cidString}</div>
            <button className="download-button" onClick={createDownloadUrl}>
              {t.downloadButtonText}
            </button>
            {downloadUrl && (
              <a href={downloadUrl} target="_blank" className="download-url">
                {`${t.downloadText} ${downloadUrl}`}
              </a>
            )}
          </div>
        )}
      </div>
    </Wrapper>
  );
}

UploadApplication.propTypes = {
  handleFileChange: PropTypes.func.isRequired,
  handleFileUpload: PropTypes.func.isRequired,
  createDownloadUrl: PropTypes.func.isRequired,
  cidString: PropTypes.string,
  wasmFile: PropTypes.any,
  downloadUrl: PropTypes.string,
};
