import React from 'react';
import styled from 'styled-components';
import { Footer } from '../footer/Footer';
import CalimeroLogo from '../../assets/calimero-logo.svg';

const Wrapper = styled.div`
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

  .content-wrapper {
    display: flex;
    flex-direction: column;
    justify-content: center;
  }
`;

export default function ContentWrapper({
  children,
}: {
  children: React.ReactNode;
}) {
  return (
    <Wrapper>
      <div className="login-navbar">
        <div className="logo-container">
          <img
            src={CalimeroLogo as unknown as string}
            alt="Calimero Admin Dashboard Logo"
            className="calimero-logo"
          />
          <h4 className="dashboard-text">Calimero Network</h4>
        </div>
      </div>
      <div className="content-card">
        <div className="content-wrapper">{children}</div>
        <Footer />
      </div>
    </Wrapper>
  );
}
