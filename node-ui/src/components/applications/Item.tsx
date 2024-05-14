import React from "react";
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
    grid-column: span 1;
    white-space: nowrap;
    overflow: hidden;
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

interface PackageItemProps {
  selectedItem: any;
}

export function PackageItem({ selectedItem }: PackageItemProps) {
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
