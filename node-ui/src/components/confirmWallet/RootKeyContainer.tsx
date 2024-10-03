import React from 'react';
import styled from 'styled-components';
import { Link } from 'react-router-dom';
import { UrlParams } from '../../utils/rootkey';
import CalimeroLogo from '../../assets/calimero-logo.svg';
import translations from '../../constants/en.global.json';
import StatusModal, { ModalContent } from '../common/StatusModal';

const Container = styled.div`
  background-color: #111111;
  height: 100vh;
  width: 100%;

  .navbar {
    display: flex;
    -webkit-box-pack: justify;
    justify-content: space-between;
    padding-top: 1rem;
    padding-bottom: 1rem;
    padding-left: 6rem;
    padding-right: 6rem;
    .logo-container {
      position: relative;
      display: flex;
      justify-content: center;
      gap: 0.5rem;
    }

    .calimero-logo {
      width: 160px;
      height: 43.3px;
    }

    .dashboard-text {
      position: absolute;
      left: 3.2rem;
      top: 2rem;
      width: max-content;
      font-size: 12px;
      color: #fff;
    }
  }

  .content-wrapper {
    display: flex;
    flex-direction: column;
    height: calc(100vh - 75.3px);
    justify-content: center;
    align-items: center;
    color: #fff;

    .content-card {
      max-width: 500px;
      overflow: hidden;
      white-space: nowrap;
      text-overflow: ellipsis;

      .content-text-title {
        width: 100%;
        text-align: center;
      }
      .param,
      .value {
        overflow: hidden;
        white-space: nowrap;
        text-overflow: ellipsis;
      }
      .button-submit {
        display: inline-flex;
        -webkit-box-align: center;
        align-items: center;
        -webkit-box-pack: center;
        justify-content: center;
        position: relative;
        box-sizing: border-box;
        -webkit-tap-highlight-color: transparent;
        outline: 0px;
        border: 0px;
        margin: 0px;
        cursor: pointer;
        user-select: none;
        vertical-align: middle;
        appearance: none;
        text-decoration: none;
        transition:
          background-color 250ms cubic-bezier(0.4, 0, 0.2, 1) 0ms,
          box-shadow 250ms cubic-bezier(0.4, 0, 0.2, 1) 0ms,
          border-color 250ms cubic-bezier(0.4, 0, 0.2, 1) 0ms,
          color 250ms cubic-bezier(0.4, 0, 0.2, 1) 0ms;
        background-color: rgb(255, 132, 45);
        box-shadow:
          rgba(0, 0, 0, 0.2) 0px 3px 1px -2px,
          rgba(0, 0, 0, 0.14) 0px 2px 2px 0px,
          rgba(0, 0, 0, 0.12) 0px 1px 5px 0px;
        min-width: 0px;
        border-radius: 8px;
        white-space: nowrap;
        color: rgb(23, 23, 29);
        font-weight: 600;
        font-size: 16px;
        line-height: 24px;
        letter-spacing: 0px;
        text-transform: uppercase;
        padding: 10px 27px;
        margin-top: 0.5rem;
      }

      .button-submit:hover {
        background-color: #ac5221;
      }

      .label {
        font-size: 12px;
        color: rgb(255, 255, 255, 0.7);
      }

      .flex-container {
        display: flex;
        justify-content: center;
        padding-top: 12px;
        width: 100%;
      }

      .back-button {
        color: rgb(255, 255, 255, 0.7);
        font-size: 16px;
        text-decoration: none;
      }

      .back-button:hover {
        color: #fff;
      }
    }
  }
`;

interface RootKeyContainerProps {
  params: UrlParams;
  submitRootKeyRequest: () => void;
  showStatusModal: boolean;
  closeStatusModal: () => void;
  addRootKeyStatus: ModalContent;
}

export function RootKeyContainer({
  params,
  submitRootKeyRequest,
  showStatusModal,
  closeStatusModal,
  addRootKeyStatus,
}: RootKeyContainerProps) {
  const t = translations.confirmWallet;
  return (
    <Container>
      <StatusModal
        show={showStatusModal}
        closeModal={closeStatusModal}
        modalContent={addRootKeyStatus}
      />
      <div className="navbar">
        <div className="logo-container">
          <img
            src={CalimeroLogo as unknown as string}
            alt="Calimero Admin Dashboard Logo"
            className="calimero-logo"
          />
          <h4 className="dashboard-text">{t.logoText}</h4>
        </div>
        <div className="logo-container">
          <img
            src={CalimeroLogo as unknown as string}
            alt="Calimero Admin Dashboard Logo"
            className="calimero-logo"
          />
          <h4 className="dashboard-text">{t.logoText}</h4>
        </div>
        <div className="logo-container">
          <img
            src={CalimeroLogo as unknown as string}
            alt="Calimero Admin Dashboard Logo"
            className="calimero-logo"
          />
          <h4 className="dashboard-text">{t.logoText}</h4>
        </div>
      </div>
      <div className="content-wrapper">
        <div>testing testing</div>
        <div className="content-card">
          <h2 className="content-text-title">{t.title}</h2>
          <div className="params-container">
            {renderParam(t.accountIdText, params.accountId)}
            {renderParam(t.signatureText, params.signature)}
            {renderParam(t.publicKeyText, params.publicKey)}
            {renderParam(t.callbackUrlText, params.callbackUrl)}
          </div>
          <div className="flex-container">
            <button className="button-submit" onClick={submitRootKeyRequest}>
              {t.submitButtonText}
            </button>
          </div>
          <div className="flex-container">
            <Link to="/" className="back-button">
              {t.backButtonText}
            </Link>
          </div>
        </div>
      </div>
    </Container>
  );
}

const renderParam = (label: string, value: string): JSX.Element => (
  <div className="param">
    <div className="label">{label}</div>
    <div className="value">{value}</div>
  </div>
);
