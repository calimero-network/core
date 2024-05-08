import React from "react";
import styled from "styled-components";
import PropTypes from "prop-types";
import CalimeroLogo from "../../assets/calimero-logo.svg";

const Wrapper = styled.div`
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

export default function CenteredWrapper({ children }) {
  return (
    <Wrapper>
      <div className="selector-wrapper">
        <div className="login-navbar">
          <div className="logo-container">
            <img
              className="calimero-logo"
              src={CalimeroLogo}
              alt="calimero-logo"
            />
            <h1 className="dashboard-text">Dashboard</h1>
          </div>
        </div>
        <div className="content-card">{children}</div>
      </div>
    </Wrapper>
  );
}

CenteredWrapper.propTypes = {
    children: PropTypes.node.isRequired,
}