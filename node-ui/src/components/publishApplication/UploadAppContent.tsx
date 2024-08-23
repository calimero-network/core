import React from 'react';
import styled from 'styled-components';
import translations from '../../constants/en.global.json';

export const ContentLayout = styled.div`
  padding: 42px 26px 26px 26px;
  display: flex;
  flex: 1;

  .content-card {
    background-color: #353540;
    border-radius: 4px;
    padding: 30px 26px 26px 30px;
    width: 100%;
    height: 100%;
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
  .button {
    border-radius: 4px;
    background-color: rgba(255, 255, 255, 0.06);
    width: fit-content;
    height: 30px;
    padding-left: 14px;
    padding-right: 14px;
    margin-top: 8px;
    cursor: pointer;
    border: none;
    outline: none;
  }
  .button:hover {
    background-color: rgba(255, 255, 255, 0.12);
  }
`;

interface UploadAppContentProps {
  addWalletAccount: () => void;
  children: React.ReactNode;
}

export function UploadAppContent({
  addWalletAccount,
  children,
}: UploadAppContentProps) {
  const t = translations.uploadPage;
  return (
    <ContentLayout>
      <div className="content-card">
        <div className="card-header">
          <div className="page-title">{t.title}</div>
          <button className="button" onClick={addWalletAccount}>
            {t.walletButtonText}
          </button>
        </div>
        {children}
      </div>
    </ContentLayout>
  );
}
