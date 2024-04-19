import React from "react";
import styled from "styled-components";
import PropTypes from "prop-types";
import translations from "../../constants/en.global.json";

export const ContentLayout = styled.div`
  display: flex;
  flex: 1;
  .card-header {
    padding: 12px;
    height: 100%;
    background-color: #121216;
    width: 100%;
  }
  .switch {
    padding-left: 24px;
    padding-right: 24px;
    padding-top: 8px;
    padding-bottom: 8px;
    border-top-left-radius: 6px;
    border-top-right-radius: 6px;
    background-color: #121216;
    width: fit-content;
    display: flex;
    color: #fff;
    font-size: 16px;
    gap: 28px;
  }
  .active {
    color: #ff842d;
  }

  .switchButton {
    cursor: pointer;
  }
`;

export function UploadSwitch({ children, setTabSwitch, tabSwitch }) {
  const t = translations.uploadPage;
  return (
    <ContentLayout>
      <div className="content-card">
        <div className="switch">
          <div
            className={`switchButton ${tabSwitch && "active"}`}
            onClick={() => setTabSwitch(true)}
          >
            {t.switchPackageText}
          </div>
          <div
            className={`switchButton ${!tabSwitch && "active"}`}
            onClick={() => setTabSwitch(false)}
          >
            {t.switchReleaseText}
          </div>
        </div>
        <div className="card-header">{children}</div>
      </div>
    </ContentLayout>
  );
}

UploadSwitch.propTypes = {
  children: PropTypes.node.isRequired,
  setTabSwitch: PropTypes.func.isRequired,
  tabSwitch: PropTypes.bool.isRequired,
};
