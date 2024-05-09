import React from "react";
import styled from "styled-components";
import PropTypes from "prop-types";
import LoaderSpinner from "../../common/LoaderSpinner";

const DetailsCardWrapper = styled.div`
  padding-left: 1rem;
  font-family: Inter;

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

export default function DetailsCard({ details }) {
  if (!details) {
    return <LoaderSpinner />;
  }

  return (
    <DetailsCardWrapper>
      <div className="container">
        <div className="context-id">id: {details.applicationId}</div>
        <div className="highlight title">Installed application</div>
        <div className="item">
          Name: <span className="highlight">{details.name}</span>
        </div>
        <div className="item">
          Owner: <span className="highlight">{details.owner}</span>
        </div>
        <div className="item">Description: {details.description}</div>
        <div className="item">
          Repository URL: {details.repository}
        </div>
        <div className="item">Version: <span className="highlight">{details.version}</span></div>
        <div className="highlight title">Storage</div>
        <div className="item">
          Used: <span className="highlight">{details.storage ?? "-"}</span>
        </div>
      </div>
    </DetailsCardWrapper>
  );
}

DetailsCard.propTypes = {
  details: PropTypes.object,
};
