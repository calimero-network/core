import React from "react";
import styled from "styled-components";
import PropTypes from "prop-types";
import translations from "../../constants/en.global.json";

export const ContentLayout = styled.div`
  padding: 42px 26px 26px 26px;
  display: flex;
  flex: 1;

  .content-card {
    background-color: #353540;
    border-radius: 4px;
    padding: 30px 26px 26px 30px;
    width: 100%;
  }

  .page-title {
    color: #fff;
    font-size: 24px;
    font-weight: semi-bold;
  }
  .card-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
  }
`;

export function UploadAppContent({ children }) {
  const t = translations.uploadPage;
  return (
    <ContentLayout>
      <div className="content-card">
        <div className="card-header">
          <div className="page-title">{t.title}</div>
        </div>
        {children}
      </div>
    </ContentLayout>
  );
}

UploadAppContent.propTypes = {
  children: PropTypes.node.isRequired,
};
