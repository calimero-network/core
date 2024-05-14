import React, { useState } from "react";
import PropTypes from "prop-types";
import CryptoWalletSelectorAnimation from "../../assets/crypto-wallet-selector-animation.svg";
import NearGif from "../../assets/near-gif.gif";
import CalimeroLogo from "../../assets/calimero-logo.svg";
import styled from "styled-components";
import translations from "../../constants/en.global.json";

const LoginWrapper = styled.div`
  background-color: #111111;
  height: 100vh;
  width: 100%;

  .login-navbar {
    display: flex;
    -webkit-box-pack: justify;
    justify-content: space-between;
    padding-top: 1rem;
    padding-bottom: 1rem;
    padding-left: 6rem;
    padding-right: 6rem;
  }

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

  .content-card {
    display: flex;
    justify-content: center;
    height: calc(100vh - 75.3px);
    align-items: center;
    color: #fff;
  }

  .wallet-logo-container {
    display: flex;
    justify-content: center;
    align-items: center;
    width: 100%;
    height: max-content;
    border-radius: 100%;
    position: relative;
  }

  .wallet-logo,
  .wallet-logo-hidden {
    height: 250px;
    width: 199px;
  }

  .wallet-logo-hidden {
    opacity: 0;
  }

  .circle-div {
    height: 250px;
    width: 250px;
    border: 1px solid #f67036;
    position: absolute;
    top: -32px;
    border-radius: 100%;
    display: flex;
    justify-content: center;
    align-items: center;
  }

  .near-gif {
    position: relative;
  }

  .content-wrapper {
    display: flex;
    flex-direction: column;
    justify-content: center;
  }
  .content-text {
    text-align: center;
  }

  .content-text-title {
    margin: 0px 0px 0.35em;
    font-weight: 600;
    font-size: 20px;
    line-height: 28px;
    letter-spacing: 0px;
    text-transform: none;
  }

  .content-text-start {
    margin: 0px 0px 0.35em;
    font-weight: 400;
    font-size: 14px;
    line-height: 21px;
    letter-spacing: 0px;
    text-transform: none;
    color: rgba(255, 255, 255, 0.7);
  }

  .button-login {
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
    transition: background-color 250ms cubic-bezier(0.4, 0, 0.2, 1) 0ms,
      box-shadow 250ms cubic-bezier(0.4, 0, 0.2, 1) 0ms,
      border-color 250ms cubic-bezier(0.4, 0, 0.2, 1) 0ms,
      color 250ms cubic-bezier(0.4, 0, 0.2, 1) 0ms;
    background-color: rgb(255, 132, 45);
    box-shadow: rgba(0, 0, 0, 0.2) 0px 3px 1px -2px,
      rgba(0, 0, 0, 0.14) 0px 2px 2px 0px, rgba(0, 0, 0, 0.12) 0px 1px 5px 0px;
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

  .button-login:hover {
    background-color: #ac5221;
  }
`;

export function Login({ verifyOwner }) {
  const t = translations.loginPage;
  const [showWallet, setShowWallet] = useState(true);

  const redirect = () => {
    setShowWallet(false);
    verifyOwner();
    setTimeout(() => {
      setShowWallet(true);
    }, 2000);
  };

  return (
    <LoginWrapper>
      <div className="login-navbar">
        <div className="logo-container">
          <img
            src={CalimeroLogo}
            alt="Calimero Admin Dashboard Logo"
            className="calimero-logo"
          />
          <h4 className="dashboard-text">{t.logoDashboardText}</h4>
        </div>
      </div>
      <div className="content-card">
        <div className="content-wrapper">
          <div className="wallet-logo-container">
            <img
              src={CryptoWalletSelectorAnimation}
              alt="Crypto Wallet Selector Animation"
              className={showWallet ? "wallet-logo" : "wallet-logo-hidden"}
            />
            <div className="circle-div">
              {!showWallet && <img src={NearGif} className="near-gif"></img>}
            </div>
          </div>
          <div className="content-text">
            <h2 className="content-text-title">{t.title}</h2>
            <span className="content-text-start">{t.subtitle}</span>
          </div>
          <button className="button-login" onClick={redirect}>
            {t.buttonConnectText}
          </button>
        </div>
      </div>
    </LoginWrapper>
  );
}

Login.propTypes = {
  verifyOwner: PropTypes.func.isRequired,
};
