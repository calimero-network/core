import React from "react";
import LoginSelector from "@calimero-is-near/calimero-p2p-sdk/lib/wallet/LoginSelector";
import { useNavigate } from "react-router-dom";
import styled from "styled-components";
import CalimeroLogo from "../assets/calimero-logo.svg";

const BootstrapWrapper = styled.div`
  height: 150px;
  .selector-wrapper {
    background-color: #121216;
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
  }
`;

export default function Login() {
  const navigate = useNavigate();
  return (
    <BootstrapWrapper>
      <div className="selector-wrapper">
        <div className="login-navbar">
          <div className="logo-container">
            <img
              src={CalimeroLogo}
              alt="Calimero Admin Dashboard Logo"
              className="calimero-logo"
            />
            <h4 className="dashboard-text">Admin Dashboard</h4>
          </div>
        </div>
        <div className="content-card">
          <LoginSelector
            navigateMetamaskLogin={() => navigate("/metamask")}
            navigateNearLogin={() => navigate("/near")}
          />
        </div>
      </div>
    </BootstrapWrapper>
  );
}
