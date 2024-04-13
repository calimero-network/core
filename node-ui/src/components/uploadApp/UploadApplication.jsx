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
`;

export function UploadApplication({
  handleFileChange,
  handleFileUpload,
  wasmFile,
  setTabSwitch,
  cidString,
  addRelease,
  packages,
}) {
  const t = translations.uploadApplication;
  const [releaseInfo, setReleaseInfo] = useState({
    name: "",
    version: "",
    notes: "",
    path: "",
    hash: "",
  });

  const getFileIPFSurl = () => {
    return `https://ipfs.io/ipfs/${cidString}`;
  };

  useEffect(() => {
    setReleaseInfo((prevState) => ({
      ...prevState,
      path: getFileIPFSurl(),
    }));
  }, [cidString]);

  return (
    <Wrapper>
      <div className="upload-form">
        <div className="title">{t.title}</div>
        <div onClick={() => setTabSwitch(false)} className="back-button">
          <ArrowLeftIcon className="arrow-icon" />
          Back
        </div>
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
      </div>
      <div className="release-info-wrapper">
        <div className="release-text">Release Information</div>
        <div className="flex-group-col">
          <label className="label-info">Name</label>
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
            <label className="label-info">Path</label>
            <input
              type="text"
              name="path"
              className="input input-name"
              value={releaseInfo.path}
              placeholder="chat-application"
              readOnly
            />
          </div>
          <div className="flex-group-col">
            <label className="label-info">Version</label>
            <input
              type="text"
              name="version"
              className="input input-name"
              value={releaseInfo.version}
              placeholder="0.0.1"
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
            <label className="label-info">Notes</label>
            <input
              type="text"
              name="notes"
              className="input input-name"
              value={releaseInfo.notes}
              placeholder="bug fix patch"
              onChange={(e) =>
                setReleaseInfo((prevState) => ({
                  ...prevState,
                  notes: e.target.value,
                }))
              }
            />
          </div>
          <div className="flex-group-col">
            <label className="label-info">Hash</label>
            <input
              type="text"
              name="hash"
              className="input input-name"
              value={releaseInfo.hash}
              placeholder="chat-application"
              onChange={(e) =>
                setReleaseInfo((prevState) => ({
                  ...prevState,
                  hash: e.target.value,
                }))
              }
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
          Add Package
        </button>
      </div>
    </Wrapper>
  );
}

UploadApplication.propTypes = {
  handleFileChange: PropTypes.func.isRequired,
  handleFileUpload: PropTypes.func.isRequired,
  wasmFile: PropTypes.any,
  cidString: PropTypes.string.isRequired,
  setTabSwitch: PropTypes.func.isRequired,
  addRelease: PropTypes.func.isRequired,
  packages: PropTypes.array,
};
