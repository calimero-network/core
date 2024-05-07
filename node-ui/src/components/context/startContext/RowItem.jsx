import React from "react";
import styled from "styled-components";
import { truncateHash } from "../../../utils/displayFunctions";

const RowItem = styled.div`
  display: flex;
  width: 100%;
  align-items: center;
  gap: 1px;
  font-size: 0.875rem;
  font-weight: 400;
  line-height: 1.25rem;
  text-align: left;
  padding-right: 1.5rem;
  padding-left: 1.5rem;
  ${(props) =>
    props.borders
      ? `
    border-top: 1px solid #23262D;
    border-bottom: 1px solid #23262D;
  `
      : `
    border-top: 1px solid #23262D;
  `}

  .row-item {
    padding: 12px 0px;
    height: 40px;
    width: 25%;
  }

  .name {
    text-align: left;
    &:hover {
        color: #4cfafc;
        cursor: pointer;
    }
  }

  .read {
    color: #9c9da3;
  }
`;

export default function rowItem(item, id, count, onRowItemClick) {
  return (
    <RowItem key={item.id} borders={id === count}>
      <div className="row-item name" onClick={() => onRowItemClick(item)}>{item.name}</div>
      <div className="row-item read">{truncateHash(item.id)}</div>
      <div className="row-item read">{item.version}</div>
      <div className="row-item read">{item.published ?? "-"}</div>
    </RowItem>
  );
}
