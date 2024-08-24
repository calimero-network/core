import React from 'react';
import styled from 'styled-components';
import translations from '../../constants/en.global.json';
import { PackageInfo } from '../../pages/PublishApplication';

const ContractFormLayout = styled.div`
  display: flex;
  flex-direction: column;
  padding-left: 1rem;
  padding-top: 2rem;
  position: relative;
  width: 100%;

  .title {
    color: #fff;
    margin-bottom: 1.5rem;
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
`;

interface AddPackageFormProps {
  packageInfo: PackageInfo;
  setPackageInfo: React.Dispatch<React.SetStateAction<PackageInfo>>;
}

export function AddPackageForm({
  packageInfo,
  setPackageInfo,
}: AddPackageFormProps) {
  const t = translations.addPackageForm;

  return (
    <ContractFormLayout>
      <div className="title">{t.title}</div>
      <label className="label">{t.nameLabelText}</label>
      <input
        type="text"
        name="name"
        className="input input-name"
        value={packageInfo.name}
        onChange={(e) =>
          setPackageInfo((prevState: PackageInfo) => ({
            ...prevState,
            name: e.target.value,
          }))
        }
      />
      <label className="label">{t.descriptionLabelText}</label>
      <input
        type="text"
        name="description"
        className="input"
        value={packageInfo.description}
        onChange={(e) =>
          setPackageInfo((prevState: PackageInfo) => ({
            ...prevState,
            description: e.target.value,
          }))
        }
      />
      <label className="label">{t.repositoryLabelText}</label>
      <input
        type="text"
        name="repository"
        className="input input-name"
        value={packageInfo.repository}
        onChange={(e) =>
          setPackageInfo((prevState: PackageInfo) => ({
            ...prevState,
            repository: e.target.value,
          }))
        }
      />
    </ContractFormLayout>
  );
}
