import React, { useEffect, useState } from "react";
import styled from "styled-components";
import PropTypes from "prop-types";
import translations from "../../constants/en.global.json";
import { ArrowLeftIcon } from "@heroicons/react/24/solid";
import Dropdown from "react-bootstrap/Dropdown";

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
      border: none;
    }

    .upload-button,
    .download-button {
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
    .upload-button:hover,
    .download-button:hover {
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
    .back-button {
      width: fit-content;
      display: flex;
      justify-content: center;
      align-items: center;
      gap: 4px;
      font-size: 14px;
      cursor: pointer;
      color: rgb(255, 255, 255, 0.7);
      .arrow-icon {
        height: 18px;
        width: 18px;
      }
    }
  }
  .release-info-wrapper {
    padding-left: 12px;
  }
  .flex-group {
    display: grid;
    grid-template-columns: repeat(2, 1fr);
    gap: 8px;
  }

  .flex-group-col {
    display: flex;
    flex-direction: column;
  }

  .release-text {
    font-size: 14px;
    color: #fff;
    margin-bottom: 12px;
  }

  .label-info {
    font-size: 12px;
    color: rgb(255, 255, 255, 0.7);
  }

  input {
    background-color: transparent;
    margin-bottom: 8px;
    padding: 8px;
    border: 1px solid rgb(255, 255, 255, 0.7);
    border-radius: 4px;
    font-size: 14px;
    color: rgb(255, 255, 255, 0.7);
    outline: none;
  }

  .input:focus {
    border: 1px solid #ff842d;
  }

  .button {
    border-radius: 4px;
    background-color: rgba(255, 255, 255, 0.06);
    width: fit-content;
    height: 30px;
    padding-left: 14px;
    padding-right: 14px;
    margin-top: 8px;
    cursor: pointer;
    border: none;
    outline: none;
  }
  .button:hover {
    background-color: rgba(255, 255, 255, 0.12);
  }

  .app-dropdown {
    background-color: #ff842d;
    border: none;
    outline: none;
    color: #111;
    font-size: 14px;
    font-weight: normal;
    width: 250px;
  }
  .dropdown-menu {
    background-color: #17171d;
    width: 250px;
  }
  .dropdown-item {
    color: #fff;
  }
  .dropdown-item:hover {
    background-color: rgb(255, 255, 255, 0.06);
  }

  .loader-wrapper {
    display: flex;
    justify-content: center;
    align-items: center;
    padding-top: 10px;
  }

  .lds-ring,
  .lds-ring div {
    box-sizing: border-box;
  }
  .lds-ring {
    display: inline-block;
    position: relative;
    width: 80px;
    height: 80px;
  }
  .lds-ring div {
    box-sizing: border-box;
    display: block;
    position: absolute;
    width: 64px;
    height: 64px;
    margin: 8px;
    border: 8px solid #121216;
    border-radius: 50%;
    animation: lds-ring 1.2s cubic-bezier(0.5, 0, 0.5, 1) infinite;
    border-color: #121216 transparent transparent transparent;
  }
  .lds-ring div:nth-child(1) {
    animation-delay: -0.45s;
  }
  .lds-ring div:nth-child(2) {
    animation-delay: -0.3s;
  }
  .lds-ring div:nth-child(3) {
    animation-delay: -0.15s;
  }
  @keyframes lds-ring {
    0% {
      transform: rotate(0deg);
    }
    100% {
      transform: rotate(360deg);
    }
  }
`;

export function UploadApplication({
  handleFileChange,
  setTabSwitch,
  ipfsPath,
  fileHash,
  addRelease,
  packages,
  addReleaseLoader,
}) {
  const t = translations.uploadApplication;
  const [releaseInfo, setReleaseInfo] = useState({
    name: "",
    version: "",
    notes: "",
    path: "",
    hash: "",
  });

  useEffect(() => {
    setReleaseInfo((prevState) => ({
      ...prevState,
      path: ipfsPath,
      hash: fileHash,
    }));
  }, [ipfsPath, fileHash]);

  return (
    <Wrapper>
      <div className="upload-form">
        <div className="title">{t.title}</div>
        <div onClick={() => setTabSwitch(false)} className="back-button">
          <ArrowLeftIcon className="arrow-icon" />
          {t.backButtonText}
        </div>
        <label className="label">{t.inputLabelText}</label>
        <input
          className="file-selection"
          type="file"
          accept=".wasm"
          onChange={handleFileChange}
        />
      </div>
      {ipfsPath && fileHash && (
        <div className="release-info-wrapper">
          <div className="release-text">{t.releaseTitle}</div>
          <div className="flex-group-col">
            <label className="label-info">{t.nameLabelText}</label>
            <Dropdown>
              <Dropdown.Toggle className="app-dropdown">
                {releaseInfo.name ? releaseInfo.name : "Select package"}
              </Dropdown.Toggle>
              <Dropdown.Menu className="dropdown-menu">
                {packages.map((pkg, id) => (
                  <Dropdown.Item
                    onClick={async () => {
                      setReleaseInfo((prevState) => ({
                        ...prevState,
                        name: pkg.name,
                      }));
                    }}
                    key={id}
                    className="dropdown-item"
                  >
                    {pkg.name}
                  </Dropdown.Item>
                ))}
              </Dropdown.Menu>
            </Dropdown>
          </div>
          <div className="flex-group">
            <div className="flex-group-col">
              <label className="label-info">{t.pathLabelText}</label>
              <input
                type="text"
                name="path"
                className="input input-name"
                value={releaseInfo.path}
                readOnly
              />
            </div>
            <div className="flex-group-col">
              <label className="label-info">{t.versionLabelText}</label>
              <input
                type="text"
                name="version"
                className="input input-name"
                value={releaseInfo.version}
                placeholder={t.versionPlaceholder}
                onChange={(e) =>
                  setReleaseInfo((prevState) => ({
                    ...prevState,
                    version: e.target.value,
                  }))
                }
              />
            </div>
          </div>
          <div className="flex-group">
            <div className="flex-group-col">
              <label className="label-info">{t.notesLabelText}</label>
              <input
                type="text"
                name="notes"
                className="input input-name"
                value={releaseInfo.notes}
                placeholder={t.notesPlaceholder}
                onChange={(e) =>
                  setReleaseInfo((prevState) => ({
                    ...prevState,
                    notes: e.target.value,
                  }))
                }
              />
            </div>
            <div className="flex-group-col">
              <label className="label-info">{t.hashLabelText}</label>
              <input
                type="text"
                name="hash"
                className="input input-name"
                value={fileHash}
                readOnly
              />
            </div>
          </div>
          <button
            className="button"
            onClick={() => addRelease(releaseInfo)}
            disabled={
              !(
                releaseInfo.version &&
                releaseInfo.notes &&
                releaseInfo.path &&
                releaseInfo.hash
              )
            }
          >
            {t.addReleaseButtonText}
          </button>

          {addReleaseLoader && (
            <div className="loader-wrapper">
              <div className="lds-ring">
                <div></div>
                <div></div>
                <div></div>
                <div></div>
              </div>
            </div>
          )}
        </div>
      )}
    </Wrapper>
  );
}

UploadApplication.propTypes = {
  handleFileChange: PropTypes.func.isRequired,
  ipfsPath: PropTypes.string.isRequired,
  fileHash: PropTypes.string.isRequired,
  setTabSwitch: PropTypes.func.isRequired,
  addRelease: PropTypes.func.isRequired,
  packages: PropTypes.array,
  addReleaseLoader: PropTypes.bool.isRequired,
};
