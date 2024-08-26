import React, { Fragment } from 'react';
import styled from 'styled-components';
import translations from '../../constants/en.global.json';
import type { AccountView } from 'near-api-js/lib/providers/provider';
import { AccountState } from '@near-wallet-selector/core/src/lib/store.types';
import LoginIcon from '../../assets/login-icon.svg';

export const Wrapper = styled.div`
  display: flex;
  flex-direction: column;
  align-items: center;
  background-color: #1c1c1c;
  width: fit-content;
  padding: 2.5rem;
  gap: 1rem;
  border-radius: 0.5rem;
  max-width: 25rem;

  .title {
    margin-top: 1.5rem;
    display: grid;
    font-size: 1.25rem;
    font-weight: 500;
    text-align: center;
    margin-bottom: 0.5rem;
    color: #fff;
  }

  .subtitle {
    display: flex;
    justify-content: start;
    align-items: center;
    font-size: 0.875rem;
    color: #778899;
    white-space: break-spaces;
  }

  .back-button {
    padding-top: 1rem;
    font-size: 0.875rem;
    color: #fff;
    text-align: center;
    cursor: pointer;
  }

  .action-button {
    background-color: #ff7a00;
    color: white;
    width: fit-content;
    display: flex;
    justify-content: center;
    align-items: center;
    gap: 0.5rem;
    height: 2.875rem;
    cursor: pointer;
    fontsize: 1rem;
    fontweight: 500;
    border-radius: 0.375rem;
    border: none;
    outline: none;
    padding-left: 0.5rem;
    padding-right: 0.5rem;
  }

  .padding-wrapper {
    display: flex;
    gap: 1rem;
  }

  .account-wrapper-padding-lg {
    margin-top: 10rem;
  }

  .account-wrapper-padding-sm {
    margin-top: 0.75rem;
  }

  .logout-button {
    background-color: rgba(26, 26, 26, 0.05);
    color: #fff;
    cursor: pointer;
    padding: 0.5rem;
    border-radius: 0.25rem;
  }

  .user-account-container {
    display: flex;
    justify-content: space-between;
    align-items: center;
    background-color: #25282d;
    height: 4.625rem;
    border-radius: 0.375rem;
    border: none;
    outline: none;
    padding: 0.25rem 0.75rem;
    width: 100%;
  }

  .center-container {
    display: flex;
    justify-content: center;
    align-items: center;
  }

  .user-account-icon-wrapper {
    display: inline-block;
    margin: 0rem;
    overflow: hidden;
    padding: 0rem;
    background-color: rgb(241, 153, 2);
    height: 1.875rem;
    width: 1.875rem;
    border-radius: 3.125rem;
  }

  .flex-column-container {
    display: flex;
    flex-direction: column;
    padding-left: 1rem;
  }

  .account-id-title {
    color: #fff;
    font-size: 0.813rem;
    line-height: 1.125rem;
    height: 1.219rem;
    font-weight:;
  }

  .account-id-value {
    color: #fff;
    font-size: 0.688rem;
    height: 1.031rem;
    font-weight: 500;
  }

  .error-message {
    color: red;
    font-size: 0.875rem;
  }
`;

export type Account = AccountView & {
  account_id: string;
};

interface NearWalletProps {
  isLogin: boolean;
  navigateBack: () => void;
  account: Account | null;
  accounts: AccountState[];
  errorMessage: string;
  handleSignout: () => void;
  handleSwitchWallet: () => void;
  handleSignMessage: () => void;
  handleSwitchAccount: () => void;
}

export default function NearWallet({
  isLogin,
  navigateBack,
  account,
  accounts,
  errorMessage,
  handleSignout,
  handleSwitchWallet,
  handleSignMessage,
  handleSwitchAccount,
}: NearWalletProps) {
  const t = isLogin
    ? translations.loginPage.nearLogin
    : translations.addRootKeyPage.nearRootKey;
  return (
    <Fragment>
      <Wrapper>
        <span className="title">{t.title}</span>
        <div className="subtitle">
          <span>{t.subtitle}</span>
        </div>
        {account && (
          <div className="user-account-container">
            <div className="center-container">
              <div className="user-account-icon-wrapper">
                <img src={LoginIcon as unknown as string} alt="login-icon" />
              </div>
              <div className="flex-column-container">
                <span className="account-id-title ">{t.accountIdText}</span>
                <span className="account-id-value">{account.account_id}</span>
              </div>
            </div>
            <div className="logout-button" onClick={handleSignout}>
              {t.logoutButtonText}
            </div>
          </div>
        )}
        <div className="error-message">{errorMessage}</div>
        <div
          className={`padding-wrapper ${account ? 'account-wrapper-padding-lg' : 'account-wrapper-padding-sm'}`}
        >
          <button className="action-button" onClick={handleSwitchWallet}>
            {t.switchWalletButtonText}
          </button>
          <button className="action-button" onClick={handleSignMessage}>
            {t.authButtonText}
          </button>
          {accounts.length > 1 && (
            <button className="action-button" onClick={handleSwitchAccount}>
              {t.switchAccountButtonText}
            </button>
          )}
        </div>
        <div className="back-button" onClick={navigateBack}>
          {t.backButtonText}
        </div>
      </Wrapper>
    </Fragment>
  );
}
