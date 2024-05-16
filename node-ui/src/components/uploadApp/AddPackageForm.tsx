import React from "react";
import styled from "styled-components";
import translations from "../../constants/en.global.json";
import Dropdown from "react-bootstrap/Dropdown";
import LoaderSpinner from "../common/LoaderSpinner";
import StatusModal from "../common/StatusModal";
import { DeployStatus, PackageInfo } from "src/pages/UploadApp";
import { Account } from "@near-wallet-selector/core";

const ContractFormLayout = styled.div`
  display: flex;
  flex-direction: column;
  padding-left: 16px;
  margin-top: 10px;
  position: relative;
  width: 100%;

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

  .flex-group {
    display: grid;
    grid-template-columns: repeat(2, 1fr);
    gap: 8px;
  }

  .flex-group-col {
    display: flex;
    flex-direction: column;
  }

  .release {
    width: 50%;
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

  .login-label {
    margin-bottom: 12px;
    margin-top: 12px;
    color: #da493f;
  }
`;

interface AddPackageFormProps {
  walletAccounts: Account[];
  addPackage: (packageInfo: PackageInfo) => void;
  addPackageLoader: boolean;
  deployerAccount: Account | null;
  setDeployerAccount: (account: Account) => void;
  showStatusModal: boolean;
  closeModal: () => void;
  deployStatus: DeployStatus;
  packageInfo: PackageInfo;
  setPackageInfo: React.Dispatch<React.SetStateAction<PackageInfo>>;
}

export function AddPackageForm({
  walletAccounts,
  addPackage,
  addPackageLoader,
  deployerAccount,
  setDeployerAccount,
  showStatusModal,
  closeModal,
  deployStatus,
  packageInfo,
  setPackageInfo
}: AddPackageFormProps) {
  const t = translations.addPackageForm;

  return (
    <ContractFormLayout>
       <StatusModal
        show={showStatusModal}
        closeModal={closeModal}
        modalContent={deployStatus}
      />
      <div className="title">{t.title}</div>
      <div className="subtitle">{t.subtitle}</div>
      <div className="flex-group">
        <div className="flex-group-col">
          {walletAccounts.length === 0 ? (
            <label className="label login-label">
             {t.loginLableText}
            </label>
          ) : (
            <>
              <label className="label">{t.deployerDropdownlabel}</label>
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
      <div className="flex-group">
        <div className="flex-group-col">
          <label className="label">{t.nameLabelText}</label>
          <input
            type="text"
            name="name"
            className="input input-name"
            value={packageInfo.name}
            placeholder={t.namePlaceholder}
            onChange={(e) =>
              setPackageInfo((prevState) => ({
                ...prevState,
                name: e.target.value,
              }))
            }
          />
        </div>
        <div className="flex-group-col">
          <label className="label">{t.repositoryLabelText}</label>
          <input
            type="text"
            name="repository"
            className="input input-name"
            value={packageInfo.repository}
            placeholder={t.repositoryPlaceholder}
            onChange={(e) =>
              setPackageInfo((prevState) => ({
                ...prevState,
                repository: e.target.value,
              }))
            }
          />
        </div>
      </div>
      <label className="label">{t.descriptionLabelText}</label>
      <input
        type="text"
        name="description"
        className="input"
        value={packageInfo.description}
        placeholder={t.descriptionPlaceholder}
        onChange={(e) =>
          setPackageInfo((prevState) => ({
            ...prevState,
            description: e.target.value,
          }))
        }
      />
      <div className="buttons-wrapper">
        <button
          className="button"
          onClick={() => addPackage(packageInfo)}
          disabled={
            !(
              packageInfo.description &&
              packageInfo.name &&
              packageInfo.repository &&
              deployerAccount
            ) && !addPackageLoader
          }
        >
          {addPackageLoader ? (
            <LoaderSpinner />
          ) : (
            <span>{t.buttonAddPackageText}</span>
          )}
        </button>
      </div>
    </ContractFormLayout>
  );
}
