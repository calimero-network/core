import React, { useState } from "react";
import styled from "styled-components";
import PropTypes from "prop-types";
import translations from "../../constants/en.global.json";

const ContractFormLayout = styled.div`
  display: flex;
  flex-direction: column;
  padding-left: 16px;
  margin-top: 10px;
  position: relative;

  .title {
    font-size: 14px;
    color: #fff;
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
  .button-next {
    position: absolute;
    bottom: 0;
    right: 0;
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

  .buttons-wrapper {
    display: flex;
    justify-content: space-between;
  }
`;

export function AddPackageForm({ addPackage, setTabSwitch, addPackageLoader }) {
  const t = translations.addPackageForm;
  const [packageInfo, setPackageInfo] = useState({
    name: "",
    description: "",
    repository: "",
  });

  return (
    <ContractFormLayout>
      <div className="title">{t.title}</div>
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
              packageInfo.repository
            ) && !addPackageLoader
          }
        >
          {t.buttonAddPackageText}
        </button>
        <button
          className="button"
          onClick={() => setTabSwitch(true)}
          disabled={addPackageLoader}
        >
          {t.buttonNextText}
        </button>
      </div>
      {addPackageLoader && (
        <div className="loader-wrapper">
          <div className="lds-ring">
            <div></div>
            <div></div>
            <div></div>
            <div></div>
          </div>
        </div>
      )}
    </ContractFormLayout>
  );
}

AddPackageForm.propTypes = {
  addPackage: PropTypes.func.isRequired,
  setTabSwitch: PropTypes.func.isRequired,
  addPackageLoader: PropTypes.bool.isRequired,
};
