import React from "react";
import styled from "styled-components";
import CalimeroLogo from "../assets/calimero-logo.svg";
import translations from "../constants/en.global.json";
import { Link } from "react-router-dom";
import { useLocation } from "react-router-dom";

const NavigationWrapper = styled.div`
  background-color: #121216;
  height: 100vh;
  width: 200px;
  padding-left: 24px;
  padding-right: 24px;
  padding-top: 12px;

  .logo-wrapper {
    display: flex;
    -webkit-box-pack: justify;
    justify-content: space-between;
  }

  .logo-container {
    position: relative;
    display: flex;
    justify-content: center;
  }

  .calimero-logo {
    width: 140px;
    height: 43.3px;
  }
  .dashboard-text {
    position: absolute;
    left: 3.2rem;
    top: 1rem;
    width: max-content;
    font-size: 10px;
    color: #fff;
  }
  .items-wrapper {
    margin-top: 24px;
    display: flex;
    flex-direction: column;
  }
  .user-container {
    background-color: #353540;
    border-radius: 8px;
    display: flex;
    justify-content: start;
    align-items: center;
    gap: 8px;
    padding-top: 8px;
    padding-bottom: 8px;
    cursor: pointer;
  }
  .user-container:hover {
    background-color: #44444f;
  }
  .user-icon {
    margin-left: 12px;
    background-color: #d48558;
    border-radius: 100%;
    height: 24px;
    width: 24px;
  }
  .separator {
    border-left: 1px solid rgba(255, 255, 255, 0.1);
    height: 44px;
  }
  .user-title {
    color: #fff;
    opacity: 0.7;
    font-size: 12px;
    font-weight: normal;
  }
  .user-pk {
    color: #fff;
    font-size: 14px;
    font-weight: medium;
    position: relative;
    top: 0px;
  }
  .text-container {
    display: flex;
    flex-direction: column;
    gap: 0px;
  }
  .navigation-items-wrapper {
    display: flex;
    flex-direction: column;
    justify-content: center;
    width: 100%;
    padding-left: 20px;
    padding-top: 24px;
    gap: 24px;
  }
  .nav-item-active {
    color: #ff842d !important;
  }
  .nav-item,
  .nav-item-active {
    color: #fff;
    font-size: 14px;
    font-weight: medium;
    cursor: pointer;
    text-decoration: none;
    display: flex;
    justify-content: start;
    align-items: center;
    gap: 4px;
  }
  .active-dot {
    width: 2px;
    height: 2px;
    border-radius: 100%;
    background-color: #ff842d;
  }
`;

const NavigationItems = [
  {
    id: 0,
    title: "Identity",
    path: "/identity",
  },
  {
    id: 1,
    title: "Applications",
    path: "/applications",
  },
  {
    id: 2,
    title: "Keys",
    path: "/keys",
  },
];

export function Navigation() {
  const t = translations.navigation;
  const location = useLocation();
  return (
    <NavigationWrapper>
      <div className="logo-wrapper">
        <div className="logo-container">
          <img
            src={CalimeroLogo}
            alt="Calimero Admin Dashboard Logo"
            className="calimero-logo"
          />
          <h4 className="dashboard-text">{t.logoDashboardText}</h4>
        </div>
      </div>
      <div className="items-wrapper">
        <div className="user-container">
          <div className="user-icon" />
          <div className="separator" />
          <div className="text-container">
            <div className="user-title">Public Key</div>
            <span className="user-pk">4rbm...Wxy3</span>
          </div>
        </div>
        <div className="navigation-items-wrapper">
          {NavigationItems.map((item) => (
            <Link
              to={item.path}
              key={item.id}
              className={
                location.pathname === item.path ? "nav-item-active" : "nav-item"
              }
            >
              <span>{item.title}</span>
              {location.pathname === item.path && (
                <div className="active-dot"></div>
              )}
            </Link>
          ))}
        </div>
      </div>
    </NavigationWrapper>
  );
}
