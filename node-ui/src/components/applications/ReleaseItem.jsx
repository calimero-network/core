import React from "react";
import PropTypes from "prop-types";
import styled from "styled-components";
import { Tooltip } from "react-tooltip";

const Item = styled.div`
  background-color: rgb(0, 0, 0, 0.12);
  padding-left: 12px;
  padding-right: 12px;
  padding-top: 10px;
  padding-bottom: 10px;
  display: grid;
  grid-template-columns: repeat(3, 1fr);
  grid-template-rows: auto;
  color: rbg(255, 255, 255, 0.9);
  font-size: 16px;

  .app-item {
    color: #fff;
    font-size: 14px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .app-item-desc,
  .app-item-repo {
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .app-item-repo {
    text-decoration: none;
    color: #ff842d;
  }

  @media (max-width: 768px) {
    .app-item-desc {
      grid-column: span 3;
    }
  }
`;

export function ReleaseItem({ release }) {
  return (
    <Item>
      <div className="item-name app-item">{`${release.hash.substring(
        0,
        4
      )}...${release.hash.substring(release.hash.length - 4)}`}</div>
      <div
        className="item-desc app-item app-item-desc"
        data-tooltip-id="my-tooltip"
      >
        {release.notes}
        {release.notes.length > 52 && (
          <Tooltip id="my-tooltip" content={release.notes} />
        )}
      </div>
      <a href={release.path} target="_blank" className="app-item app-item-repo">
        {release.path}
      </a>
    </Item>
  );
}

ReleaseItem.propTypes = {
  release: PropTypes.object.isRequired,
};
