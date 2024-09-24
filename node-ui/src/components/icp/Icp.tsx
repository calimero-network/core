import React, { Fragment } from 'react';
import Loading from '../common/Loading';
import { WalletSignatureData } from '../../api/dataSource/NodeDataSource';
import { styled } from 'styled-components';
import translations from '../../constants/en.global.json';
import { NetworkId } from '../../hooks/useIcp';

const Wrapper = styled.div`
  display: flex;
  flex-direction: column;
  align-items: center;
  padding: 0.5rem;
  max-width: 400px;

  .wrapper {
    margin-top: 1.5rem;
    display: grid;
    color: #ffffff;
    font-size: 1.25rem;
    font-weight: 500;
    text-align: center;

    .title {
      margin-bottom: 0.5rem;
      color: #ffffff;
    }

    .subtitle-wrapper {
      display: flex;
      justify-content: center;
      align-items: center;
      font-size: 1.4rem;
      color: #778899;
      white-space: break-spaces;

      .subtitle-content {
        font-size: 0.875rem;
        color: #778899;
        white-space: break-spaces;
      }
    }

    .wallet-options-wrapper {
      display: flex;
      align-items: center;
      justify-content: space-between;
      margin-top: 1.5rem;
      min-width: 500px;

      .wallet-btn {
        margin-top: 1.5rem;
        display: grid;
        font-size: 1.25rem;
        font-weight: 500;
        text-align: center;
        margin-bottom: 0.5rem;
        color: #000000;
        background-color: #ffffff;
        padding: 0.5rem 0.7rem;
        cursor: pointer;
        border-radius: 0.375rem;
      }
    }
    .metamask-network {
      margin-top: 1.5rem;
      display: flex;
      align-items: center;
      justify-content: center;
      flex-direction: column;

      label {
        font-size: 1rem;
        margin-right: auto;
      }

      select {
        margin-top: 0.5rem;
        width: 100%;
        height: 46px;
        font-size: 1rem;
        font-weight: 500;
        border-radius: 0.375rem;
        padding: 0.5rem;
      }
    }

    .sign-wrapper {
      margin-top: 1.5rem;
      display: flex;
      align-items: center;
      justify-content: center;
      flex-direction: column;

      .sign-btn {
        background-color: #ff7a00;
        color: #ffffff;
        width: 100%;
        display: flex;
        justify-content: center;
        align-items: center;
        gap: 0.5rem;
        height: 46px;
        cursor: pointer;
        font-size: 1rem;
        font-weight: 500;
        border-radius: 0.375rem;
        border: none;
        outline: none;
        padding-right: 0.5rem;
        padding-left: 0.5rem;
      }
    }
    .logout-wrapper {
      padding-top: 1rem;
      font-size: 14px;
      color: #ffffff;
      text-align: center;
      cursor: pointer;
    }

    .back-wrapper {
      padding-top: 1rem;
      font-size: 14px;
      color: #ffffff;
      text-align: center;
      cursor: pointer;
    }

    .error-message {
      color: red;
      font-size: 14px;
      font-weight: 500;
      margin-top: 0.5rem;
    }
  }
`;

interface LoginWithIIProps {
  navigateBack: () => void;
  isLogin: boolean;
  ready: boolean;
  walletSignatureData: WalletSignatureData | null;
  signMessageAndLogin: (
    isLogin: boolean,
    setErrorMessage: (msg: string) => void,
  ) => void;
  errorMessage: string;
  setErrorMessage: (msg: string) => void;
  logout: (setErrorMessage: (msg: string) => void) => void;
  changeNetwork: (
    networkId: NetworkId,
    setErrorMessage: (msg: string) => void,
  ) => void;
}

export function Icp({
  navigateBack,
  isLogin,
  ready,
  walletSignatureData,
  signMessageAndLogin,
  errorMessage,
  setErrorMessage,
  changeNetwork,
}: LoginWithIIProps) {
  const t = isLogin
    ? translations.loginPage.IcpLogin
    : translations.addRootKeyPage.IcpKey;

  if (!ready) {
    return (
      <Loading loaderColor={'#FF7A00'} loaderSize={'48px'} borderSize={'5px'} />
    );
  }

  return (
    <Fragment>
      <Wrapper>
        <div className="wrapper">
          <span className="title">{t.title}</span>
          <div className="subtitle-wrapper">
            <span className="subtitle-content">{t.subtitle}</span>
          </div>
          <div className="metamask-network">
            <label htmlFor="network">{t.currentNetwork}:</label>
            <select
              name="network"
              defaultValue="production"
              onChange={(e) =>
                changeNetwork(e.target.value as NetworkId, setErrorMessage)
              }
            >
              <option value="production">Production</option>
              <option value="staging">Staging</option>
            </select>
          </div>
          {walletSignatureData && (
            <>
              <div className="sign-wrapper">
                <button
                  className="sign-btn"
                  onClick={() => signMessageAndLogin(isLogin, setErrorMessage)}
                >
                  {t.authButtonText}
                </button>
              </div>
            </>
          )}
          <div className="back-wrapper" onClick={() => navigateBack()}>
            {t.backToLoginPage}
          </div>
          {errorMessage && <div className="error-message">{errorMessage}</div>}
        </div>
      </Wrapper>
    </Fragment>
  );
}
