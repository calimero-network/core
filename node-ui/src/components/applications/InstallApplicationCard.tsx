import React from 'react';
import styled from 'styled-components';

import { XMarkIcon } from '@heroicons/react/24/solid';
import { Application } from '../../pages/InstallApplication';
import Button from '../common/Button';
import ApplicationsPopup from '../context/startContext/ApplicationsPopup';
import StatusModal, { ModalContent } from '../common/StatusModal';
import translations from '../../constants/en.global.json';

const Wrapper = styled.div`
  display: flex;
  flex: 1;
  flex-direction: column;
  padding: 1rem;
  font-optical-sizing: auto;
  font-weight: 500;
  font-style: normal;
  font-variation-settings: 'slnt' 0;
  -webkit-font-smoothing: antialiased;
  -moz-osx-font-smoothing: grayscale;
  font-smooth: never;

  .section-title {
    font-size: 0.875rem;
    font-weight: 500;
    line-height: 1.25rem;
    text-align: left;
    color: #6b7280;
  }

  .cancel-icon {
    position: relative;
    right: -0.25rem;
    cursor: pointer;
    height: 1.25rem;
    width: 1.25rem;
    color: #fff;
    cursor: pointer;
    &:hover {
      color: #4cfafc;
    }
  }

  .select-app-section {
    .button-container {
      display: flex;
      padding-top: 1rem;
      gap: 1rem;
    }

    .selected-app {
      display: flex;
      flex-direction: column;
      padding-top: 0.25rem;
      padding-left: 0.5rem;
      font-size: 0.875rem;
      font-weight: 500;
      line-height: 1.25rem;
      text-align: left;
      .label {
        color: #6b7280;
      }
      .value {
        color: #fff;
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
        height: 12.5rem;
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

interface InstallApplicationCardProps {
  application: Application;
  setApplication: (application: Application) => void;
  installApplication: () => void;
  showBrowseApplication: boolean;
  setShowBrowseApplication: (show: boolean) => void;
  onUploadClick: () => void;
  isLoading: boolean;
  showStatusModal: boolean;
  closeModal: () => void;
  installAppStatus: ModalContent;
}

export default function InstallApplicationCard({
  application,
  setApplication,
  installApplication,
  showBrowseApplication,
  setShowBrowseApplication,
  onUploadClick,
  isLoading,
  showStatusModal,
  closeModal,
  installAppStatus,
}: InstallApplicationCardProps) {
  const t = translations.applicationsPage.installApplication;
  const onStartContextClick = async () => {
    if (!application.appId) {
      return;
    }
    installApplication();
  };

  return (
    <Wrapper>
      <StatusModal
        show={showStatusModal}
        closeModal={closeModal}
        modalContent={installAppStatus}
      />
      {showBrowseApplication && (
        <ApplicationsPopup
          show={showBrowseApplication}
          closeModal={() => setShowBrowseApplication(false)}
          setApplication={setApplication}
        />
      )}
      <div className="select-app-section">
        <div className="section-title">
          {application.appId
            ? t.selectedApplicationTitle
            : t.selectApplicationTitle}
          {application.appId && (
            <XMarkIcon
              className="cancel-icon"
              onClick={() =>
                setApplication({
                  appId: '',
                  name: '',
                  version: '',
                  path: '',
                  hash: '',
                })
              }
            />
          )}
        </div>
        {application.appId ? (
          <div className="selected-app">
            <p className="label">
              {t.idLabelText}
              <span className="value">{application.appId}</span>
            </p>
            <p className="label">
              {t.nameLabelText}
              <span className="value">{application.name}</span>
            </p>
            <p className="label">
              {t.versionLabelText}
              <span className="value">{application.version}</span>
            </p>
          </div>
        ) : (
          <div className="button-container">
            <Button
              text="Browse"
              width={'144px'}
              onClick={() => setShowBrowseApplication(true)}
            />
            <Button text="Upload" width={'144px'} onClick={onUploadClick} />
          </div>
        )}
      </div>
      <div className="init-section">
        <Button
          text="Start"
          width={'144px'}
          onClick={onStartContextClick}
          isLoading={isLoading}
        />
      </div>
    </Wrapper>
  );
}
