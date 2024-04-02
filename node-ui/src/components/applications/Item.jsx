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
  display: flex;
  gap: 40px;
  color: rbg(255, 255, 255, 0.9);
  font-size: 16px;
  .app-item {
    color: #fff;
    font-size: 14px;
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
`;

export function PackageItem({ selectedItem }) {
  return (
    <Item>
      <div className="item-name app-item">{selectedItem.name}</div>
      <div
        className="item-desc app-item app-item-desc"
        data-tooltip-id="my-tooltip"
      >
        {selectedItem.description}
        {selectedItem.description.length > 52 && (
          <Tooltip id="my-tooltip" content={selectedItem.description} />
        )}
      </div>
      <a
        href={selectedItem.repository}
        target="_blank"
        className="app-item app-item-repo"
      >
        {selectedItem.repository}
      </a>
    </Item>
  );
}

PackageItem.propTypes = {
  selectedItem: PropTypes.object.isRequired,
};
