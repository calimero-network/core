import React, { Fragment } from 'react';
import { MetaMaskButton } from '@metamask/sdk-react-ui';
import Loading from '../common/Loading';
import { WalletSignatureData } from '../../api/dataSource/NodeDataSource';
import { styled } from 'styled-components';
import translations from '../../constants/en.global.json';

const Wrapper = styled.div`
  display: flex;
  flex-direction: column;
  align-items: center;
  background-color: #1c1c1c;
  width: fit-content;
  padding: 2.5rem;
  gap: 1rem;
  border-radius: 0.5rem;
  max-width: 25rem;

  display: flex;
  flex-direction: column;
  color: white;
  font-size: 1.25rem;
  font-weight: 500;
  text-align: center;

  .title {
    margin-bottom: 0.5rem;
    color: #fff;
  }

  .subtitle {
    display: flex;
    justify-content: start;
    align-items: center;
    font-size: 14px;
    color: #778899;
    white-space: break-spaces;
  }

  .header {
    margin-top: 1.5rem;
    display: flex;
    flex-direction: column;
  }

  .options-wrapper {
    margin-top: 8rem;
    width: 100%;

    .button-sign {
      background-color: #ff7a00;
      color: white;
      width: 100%;
      display: flex;
      justify-content: center;
      align-items: center;
      gap: 0.5rem;
      height: 2.875rem;
      cursor: pointer;
      font-size: 1rem;
      font-weight: 500;
      border-radius: 0.375rem;
      border: none;
      outline: none;
      padding-left: 0.5rem;
      padding-right: 0.5rem;
    }

    .error-title {
      color: red;
      font-size: 0.875rem;
      font-weight: 500;
      margin-top: 0.5rem;
    }

    .error-message {
      color: red;
      font-size: 0.875rem;
      font-weight: 500;
      margin-top: 0.5rem;
    }
  }

  .button-back {
    padding-top: 1rem;
    font-size: 0.875rem;
    color: #fff;
    text-align: center;
    cursor: pointer;
  }
`;

const connectedButtonStyle = {
  display: 'flex',
  justifyContent: 'center',
  alignItems: 'center',
  backgroundColor: '#25282D',
  height: '73px',
  borderRadius: '6px',
  border: 'none',
  outline: 'none',
};

const disconnectedButtonStyle = {
  cursor: 'pointer',
};

interface LoginWithMetamaskProps {
  navigateBack: () => void | undefined;
  ready: boolean;
  isConnected: boolean;
  walletSignatureData: WalletSignatureData | null;
  isSignLoading: boolean;
  signMessage: () => void;
  isSignError: boolean;
  errorMessage: string;
  isLogin: boolean;
}

export function MetamaskWallet({
  navigateBack,
  ready,
  isConnected,
  walletSignatureData,
  isSignLoading,
  signMessage,
  isSignError,
  errorMessage,
  isLogin,
}: LoginWithMetamaskProps) {
  const t = translations.loginPage.metamaskLogin;

  if (!ready) {
    return <Loading />;
  }

  return (
    <Fragment>
      <Wrapper>
        <span className="title">{isLogin ? t.titleLogin : t.titleRootKey}</span>
        <div className="subtitle">
          <span>{t.subtitle}</span>
        </div>
        <header className="header">
          <MetaMaskButton
            theme="dark"
            color={isConnected && walletSignatureData ? 'blue' : 'white'}
            buttonStyle={
              isConnected && walletSignatureData
                ? connectedButtonStyle
                : disconnectedButtonStyle
            }
          />
          {isConnected && walletSignatureData && (
            <div className="options-wrapper">
              <button
                className="button-sign"
                disabled={isSignLoading}
                onClick={() => signMessage()}
              >
                {t.authButtonText}
              </button>
              {isSignError && (
                <div className="error-title">{t.errorTitleText}</div>
              )}
              <div className="error-message">{errorMessage}</div>
            </div>
          )}
        </header>
        <div className="button-back" onClick={navigateBack}>
          {t.backButtonText}
        </div>
      </Wrapper>
    </Fragment>
  );
}
