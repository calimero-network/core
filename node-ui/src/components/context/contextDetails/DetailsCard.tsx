import React from "react";
import styled from "styled-components";
import LoaderSpinner from "../../common/LoaderSpinner";
import translations from "../../../constants/en.global.json";
import { ContextObject } from "../../../pages/ContextDetails";

const DetailsCardWrapper = styled.div`
  padding-left: 1rem;

  .container {
    padding: 1rem;
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
  details: ContextObject;
}

export default function DetailsCard({ details }: DetailsCardProps) {
  const t = translations.contextPage.contextDetails;

  if (!details) {
    return <LoaderSpinner />;
  }

  return (
    <DetailsCardWrapper>
      <div className="container">
        <div className="context-id">
          {t.labelIdText}
          {details.applicationId}
        </div>
        <div className="highlight title inter-mid">{t.titleApps}</div>
        <div className="item">
          {t.labelNameText}
          <span className="highlight">{details.name}</span>
        </div>
        <div className="item">
          {t.labelOwnerText}
          <span className="highlight">{details.owner}</span>
        </div>
        <div className="item">
          {t.labelDescriptionText}
          {details.description}
        </div>
        <div className="item">
          {t.labelRepositoryText}
          {details.repository}
        </div>
        <div className="item">
          {t.lableVersionText}
          <span className="highlight">{details.version}</span>
        </div>
        <div className="highlight title">{t.titleStorage}</div>
        <div className="item">
          {t.labelStorageText}
          <span className="highlight">{"-"}</span>
        </div>
      </div>
    </DetailsCardWrapper>
  );
}
