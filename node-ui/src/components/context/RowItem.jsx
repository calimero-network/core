import React from "react";
import styled from "styled-components";

const RowItem = styled.div`
  display: flex;
  align-items: center;
  ${(props) =>
    props.$borders
      ? `
    border-top: 1px solid #23262D;
    border-bottom: 1px solid #23262D;
  ` : `
    border-top: 1px solid #23262D;
  `}

  .row-item {
    width: 50%;
    padding: 0.75rem 0rem;
    height: 2.5rem;

  }
  .id {
    padding-left: 1.5rem;
    font-family: Inter;
    font-size: 0.875rem
    font-weight: 500;
    line-height: 1.25rem;
    text-align: left;
    color: #fff;
    text-decoration: none;

    &:hover {
      color: #76f5f9;
    }
  }

  .name {
    color: #6B7280;
    display: flex; 
    jusitify-content: center;
    align-items: center;
  }
`;

export default function rowItem(item, id, count) {
  return (
    <RowItem key={item.id} $borders={id === count}>
      <a href={`/admin/context/${item.id}`} className="row-item id">
        {item.id}
      </a>
      <div className="row-item name">{item.name}</div>
    </RowItem>
  );
}
