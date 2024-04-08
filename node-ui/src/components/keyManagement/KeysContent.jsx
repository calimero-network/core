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
`;

export function KeysContent({ children }) {
  const t = translations.keysPage;
  return (
    <ContentLayout>
      <div className="content-card">
        <div className="page-title">{t.title}</div>
        {children}
      </div>
    </ContentLayout>
  );
}

KeysContent.propTypes = {
  children: PropTypes.node.isRequired,
};
