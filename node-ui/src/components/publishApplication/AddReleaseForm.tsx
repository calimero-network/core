import React from "react";
import styled from "styled-components";
import translations from "../../constants/en.global.json";
import { ReleaseInfo } from "../../pages/PublishApplication";

const Wrapper = styled.div`
  width: 100%;
  flex: 1;
  padding-left: 16px;
  margin-top: 10px;
  display: flex;
  flex-direction: column;

  .title {
    color: #fff;
    margin-bottom: 1rem;
    font-size: 1rem;
    font-weight: 500;
    line-height: 1.25rem;
    text-align: left;
  }

  .label {
    color: rgb(255, 255, 255, 0.4);
    font-size: 0.625rem;
    font-weight: 500;
    line-height: 0.75rem;
    text-align: left;
    margin-bottom: 1rem;
  }

  input {
    background-color: transparent;
    margin-bottom: 1rem;
    padding: 0.5rem;
    border: 1px solid rgb(255, 255, 255, 0.1);
    background-color: rgb(255, 255, 255, 0.2);
    border-radius: 0.25rem;
    font-size: 0.875rem;
    color: rgb(255, 255, 255, 0.7);
    outline: none;
    width: 60%;
  }

  .input:focus {
    border: 1px solid #4cfafc;
  }

  .inputWrapper {
    margin-bottom: 1rem;
    display: flex;
    justify-content: center;
    align-items: center;
    height: 2.375rem;
    width: 11.375rem;
    overflow: hidden;
    position: relative;
    cursor: pointer;
    padding: 0.625rem 0.75rem;
    border-radius: 0.375rem;
    color: #000;
    font-size: 0.875rem;
    font-weight: 500;
    line-height: 1.25rem;
    text-align: center;
    cursor: pointer;
    background-color: #4cfafc;

    &:hover {
      background-color: #76f5f9;
    }
  }

  .fileInput {
    cursor: pointer;
    height: 100%;
    width: 100%;
    position: absolute;
    top: 0;
    right: 0;
    z-index: 99;
    opacity: 0;
    -moz-opacity: 0;
    filter: progid:DXImageTransform.Microsoft.Alpha(opacity=0);
  }
`;

interface AddReleaseFormProps {
  handleFileChange: (e: React.ChangeEvent<HTMLInputElement>) => void;
  fileHash: string;
  releaseInfo: ReleaseInfo;
  setReleaseInfo: React.Dispatch<React.SetStateAction<ReleaseInfo>>;
  fileInputRef: React.RefObject<HTMLInputElement>;
}

export function AddReleaseForm({
  handleFileChange,
  fileHash,
  releaseInfo,
  setReleaseInfo,
  fileInputRef,
}: AddReleaseFormProps) {
  const t = translations.uploadApplication;

  return (
    <Wrapper>
      <div className="title">{t.releaseTitle}</div>

      <div className="inputWrapper">
        <input
          className="fileInput"
          name="file"
          type="file"
          accept=".wasm"
          onChange={handleFileChange}
          ref={fileInputRef}
          data-buttonText="Upload wasm"
        />
        {t.buttonUploadLabel}
      </div>

      <label className="label">{t.pathLabelText}</label>
      <input
        type="text"
        name="path"
        className="input input-name"
        value={releaseInfo.path}
        readOnly
      />

      <label className="label">{t.versionLabelText}</label>
      <input
        type="text"
        name="version"
        className="input input-name"
        value={releaseInfo.version}
        onChange={(e) =>
          setReleaseInfo((prevState: ReleaseInfo) => ({
            ...prevState,
            version: e.target.value,
          }))
        }
      />
      <label className="label">{t.notesLabelText}</label>
      <input
        type="text"
        name="notes"
        className="input input"
        value={releaseInfo.notes}
        onChange={(e) =>
          setReleaseInfo((prevState: ReleaseInfo) => ({
            ...prevState,
            notes: e.target.value,
          }))
        }
      />
      <label className="label">{t.hashLabelText}</label>
      <input
        type="text"
        name="hash"
        className="input input"
        value={fileHash}
        readOnly
      />
    </Wrapper>
  );
}
