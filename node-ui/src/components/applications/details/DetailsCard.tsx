import React from 'react';
import styled from 'styled-components';
import LoaderSpinner from '../../common/LoaderSpinner';
import translations from '../../../constants/en.global.json';
import { Package } from '../../../pages/Applications';

const DetailsCardWrapper = styled.div`
  .container {
    display: flex;
    flex-direction: column;

    .title {
      padding-top: 1rem;
      padding-bottom: 1rem;
    }

    .context-id,
    .highlight,
    .item {
      font-size: 1rem;
      line-height: 1.25rem;
      text-align: left;
    }

    .context-id {
      font-weight: 400;
      color: #6b7280;
    }

    .highlight {
      font-weight: 500;
      color: #fff;
    }

    .item {
      font-weight: 500;
      color: #6b7280;
      padding-bottom: 4px;
    }
  }
`;

interface DetailsCardProps {
  details: Package;
}

export default function DetailsCard({ details }: DetailsCardProps) {
  const t = translations.contextPage.contextDetails;

  if (!details) {
    return <LoaderSpinner />;
  }

  return (
    <DetailsCardWrapper>
      <div className="container">
        <div className="item">
          {t.labelNameText}
          <span className="highlight">{details.name}</span>
        </div>
        <div className="item">
          {t.labelIdText}
          <span className="highlight">{details.id}</span>
        </div>
        <div className="item">
          {t.labelOwnerText}
          <span className="highlight">{details.owner || '-'}</span>
        </div>
        <div className="item">
          {t.labelDescriptionText}
          {details.description || '-'}
        </div>
        <div className="item">
          {t.labelRepositoryText}
          {details.repository || '-'}
        </div>
      </div>
    </DetailsCardWrapper>
  );
}
