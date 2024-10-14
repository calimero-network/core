import React from 'react';
import styled from 'styled-components';
import LoaderSpinner from '../../common/LoaderSpinner';
import translations from '../../../constants/en.global.json';
import { convertBytes } from '../../../utils/displayFunctions';
import { ContextStorage } from '../../../api/dataSource/NodeDataSource';
import { ContextDetails } from '../../../types/context';

const DetailsCardWrapper = styled.div`
  padding-left: 1rem;

  .container,
  .container-full {
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
  .container-full {
    display: flex;
    align-items: center;
  }
`;

interface DetailsCardProps {
  details: ContextDetails;
  detailsErrror: string | null;
  contextStorage: ContextStorage;
  contextStorageError: string | null;
}

export default function DetailsCard(props: DetailsCardProps) {
  const t = translations.contextPage.contextDetails;

  if (!props.details) {
    return <LoaderSpinner />;
  }

  return (
    <DetailsCardWrapper>
      {props.details ? (
        <div className="container">
          <div className="context-id">
            {t.labelIdText}
            {props.details.contextId}
          </div>
          {!props.details.package ? (
            <div className="highlight title inter-mid">{t.localAppTitle}</div>
          ) : (
            <div className="highlight title inter-mid">{t.titleApps}</div>
          )}
          <div className="item">
            {t.labelNameText}
            <span className="highlight">
              {props.details.package?.name ?? '-'}
            </span>
          </div>
          <div className="item">
            {t.labelOwnerText}
            <span className="highlight">
              {props.details.package?.owner ?? '-'}
            </span>
          </div>
          <div className="item">
            {t.labelDescriptionText}
            {props.details.package?.description ?? '-'}
          </div>
          <div className="item">
            {t.labelRepositoryText}
            {props.details.package?.repository ?? '-'}
          </div>
          <div className="item">
            {t.lableVersionText}
            <span className="highlight">
              {props.details.release?.version ?? '-'}
            </span>
          </div>
          <div className="item">
            {t.labelAppId}
            {props.details.applicationId}
          </div>
          <div className="highlight title">{t.titleStorage}</div>
          <div className="item">
            {t.labelStorageText}
            <span className="highlight">
              {props.contextStorage
                ? convertBytes(props.contextStorage.sizeInBytes)
                : props.contextStorageError}
            </span>
          </div>
        </div>
      ) : (
        <div className="container-full">
          <div className="item">{props.details}</div>
        </div>
      )}
    </DetailsCardWrapper>
  );
}
