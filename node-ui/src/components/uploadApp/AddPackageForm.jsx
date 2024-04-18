import React, { useState } from "react";
import styled from "styled-components";
import PropTypes from "prop-types";
import translations from "../../constants/en.global.json";
import LoaderSpinner from "../common/LoaderSpinner";

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
`;

export function AddPackageForm({ addPackage, addPackageLoader }) {
  const t = translations.addPackageForm;
  const [packageInfo, setPackageInfo] = useState({
    name: "",
    description: "",
    repository: "",
  });

  return (
    <ContractFormLayout>
      <div className="title">{t.title}</div>
      <div className="subtitle">{t.subtitle}</div>
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

AddPackageForm.propTypes = {
  addPackage: PropTypes.func.isRequired,
  addPackageLoader: PropTypes.bool.isRequired,
};
