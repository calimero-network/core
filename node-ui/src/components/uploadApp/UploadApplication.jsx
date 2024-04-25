import React from "react";
import styled from "styled-components";
import PropTypes from "prop-types";
import translations from "../../constants/en.global.json";
import Dropdown from "react-bootstrap/Dropdown";
import LoaderSpinner from "../common/LoaderSpinner";
import StatusModal from "../common/StatusModal";

const Wrapper = styled.div`
  width: 100%;
  padding-left: 16px;
  margin-top: 10px;
  .upload-form {
    display: flex;
    flex-direction: column;

    .title {
      font-size: 14px;
      color: #fff;
      margin-bottom: 12px;
    }

    .subtitle {
      font-size: 12px;
      color: rgb(255, 255, 255, 0.7);
      margin-bottom: 12px;
    }

    .label {
      font-size: 12px;
      color: rgb(255, 255, 255, 0.7);
    }

    .file-selection {
      margin-top: 8px;
      color: #fff;
      display: flex;
      gap: 12px;
      width: fit-content;
      border: none;
      padding: 0px;
    }

    .upload-button {
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

    .upload-button:hover {
      background-color: rgba(255, 255, 255, 0.12);
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
  }

  .release-info-wrapper {
    padding-right: 12px;
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
  .dropdown-menu-scroll {
    height: 200px;
    overflow-y: scroll;
  }
  .dropdown-item {
    color: #fff;
  }
  .dropdown-item:hover {
    background-color: rgb(255, 255, 255, 0.06);
  }

  .button {
    border-radius: 4px;
    background-color: rgba(255, 255, 255, 0.06);
    width: 140px;
    height: 30px;
    padding-left: 14px;
    padding-right: 14px;
    margin-top: 8px;
    cursor: pointer;
    border: none;
    outline: none;
    diplay: flex;
    justify-content: center;
    align-items: center;
  }
  .button:hover {
    background-color: rgba(255, 255, 255, 0.12);
  }
  .buttons-wrapper {
    display: flex;
    justify-content: space-between;
  }
  .wallet-select {
    margin-bottom: 12px;
  }
  .login-label {
    margin-bottom: 12px;
    margin-top: 12px;
    color: #da493f;
  }
`;

export function UploadApplication({
  handleFileChange,
  ipfsPath,
  fileHash,
  addRelease,
  packages,
  addReleaseLoader,
  walletAccounts,
  deployerAccount,
  setDeployerAccount,
  showStatusModal,
  closeModal,
  deployStatus,
  releaseInfo,
  setReleaseInfo,
  fileInputRef,
}) {
  const t = translations.uploadApplication;

  return (
    <Wrapper>
      <StatusModal
        show={showStatusModal}
        closeModal={closeModal}
        modalContent={deployStatus}
      />
      <div className="upload-form">
        <div className="title">{t.title}</div>
        <label className="subtitle">{t.subtitle}</label>
        <div className="title">{t.uploadIpfsTitle}</div>
        <label className="label">{t.inputLabelText}</label>
        <input
          className="file-selection"
          type="file"
          accept=".wasm"
          onChange={handleFileChange}
          ref={fileInputRef}
        />
      </div>
      {ipfsPath && fileHash && (
        <div className="release-info-wrapper">
          <div className="flex-group">
            <div className="flex-group-col wallet-select">
              {walletAccounts.length === 0 ? (
                <label className="label-info login-label">
                  {t.loginLableText}
                </label>
              ) : (
                <>
                  <label className="label-info">
                    {t.deployerDropdownlabel}
                  </label>
                  <Dropdown>
                    <Dropdown.Toggle className="app-dropdown">
                      {deployerAccount
                        ? deployerAccount.accountId
                        : t.deployerDropdownText}
                    </Dropdown.Toggle>
                    <Dropdown.Menu className="dropdown-menu">
                      {walletAccounts?.map((account, id) => (
                        <Dropdown.Item
                          key={id}
                          className="dropdown-item"
                          onClick={() => setDeployerAccount(account)}
                        >
                          {account.accountId}
                        </Dropdown.Item>
                      ))}
                    </Dropdown.Menu>
                  </Dropdown>
                </>
              )}
            </div>
          </div>
          <div className="release-text">{t.releaseTitle}</div>
          <div className="flex-group-col">
            <label className="label-info">{t.nameLabelText}</label>
            <Dropdown>
              <Dropdown.Toggle className="app-dropdown">
                {releaseInfo.name ? releaseInfo.name : t.selectPackageLabel}
              </Dropdown.Toggle>
              <Dropdown.Menu className="dropdown-menu dropdown-menu-scroll">
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
          <div className="buttons-wrapper">
            <button
              className="button"
              onClick={() => addRelease(releaseInfo)}
              disabled={
                !(
                  releaseInfo.version &&
                  releaseInfo.notes &&
                  releaseInfo.path &&
                  releaseInfo.hash &&
                  releaseInfo.name &&
                  deployerAccount
                )
              }
            >
              {addReleaseLoader ? (
                <LoaderSpinner />
              ) : (
                <span>{t.addReleaseButtonText}</span>
              )}
            </button>
          </div>
        </div>
      )}
    </Wrapper>
  );
}

UploadApplication.propTypes = {
  handleFileChange: PropTypes.func.isRequired,
  ipfsPath: PropTypes.string.isRequired,
  fileHash: PropTypes.string.isRequired,
  addRelease: PropTypes.func.isRequired,
  packages: PropTypes.array,
  addReleaseLoader: PropTypes.bool.isRequired,
  walletAccounts: PropTypes.array.isRequired,
  deployerAccount: PropTypes.object,
  setDeployerAccount: PropTypes.func.isRequired,
  showStatusModal: PropTypes.bool.isRequired,
  closeModal: PropTypes.func.isRequired,
  deployStatus: PropTypes.object.isRequired,
  releaseInfo: PropTypes.object.isRequired,
  setReleaseInfo: PropTypes.func.isRequired,
  fileInputRef: PropTypes.object,
};
