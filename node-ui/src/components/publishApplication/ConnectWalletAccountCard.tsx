import React from 'react';
import styled from 'styled-components';
import Button from '../common/Button';
import translations from '../../constants/en.global.json';

const CardWrapper = styled.div`
  display: flex;
  flex-direction: column;
  gap: 1rem;
  padding-left: 1rem;

  .title {
    color: #fff;
    font-size: 1rem;
    font-weight: 500;
    line-height: 1.25rem;
    text-align: left;
  }

  .subtitle {
    color: rgb(255, 255, 255, 0.4);
    font-size: 0.625rem;
    font-weight: 500;
    line-height: 0.75rem;
    text-align: left;
  }
`;

interface ConnectWalletAccountCardProps {
  onClick: () => void;
}

export function ConnectWalletAccountCard({
  onClick,
}: ConnectWalletAccountCardProps) {
  const t = translations.applicationsPage.publishApplication;
  return (
    <CardWrapper>
      <div className="title">{t.connectAccountTitle}</div>
      <div className="subtitle">{t.connectAccountSubtitle}</div>
      <Button
        onClick={onClick}
        text={t.connectAccountButtonText}
        width="11.375rem"
      />
    </CardWrapper>
  );
}
