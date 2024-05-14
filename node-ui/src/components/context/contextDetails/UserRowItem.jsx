import React from "react";
import styled from "styled-components";

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
    props.$borders
      ? `
    border-top: 1px solid #23262D;
    border-bottom: 1px solid #23262D;
  `
      : `
    border-top: 1px solid #23262D;
  `}

  .row-item {
    padding: 0.75rem 0rem;
    height: 2.5rem;
    width: 50%;
    color: #fff;
  }
`;

export default function userRowItem(item, id, count) {
  return (
    <RowItem key={item.userId} $borders={id === count}>
      <div className="row-item">{item.userId}</div>
      <div className="row-item">{item.joined}</div>
    </RowItem>
  );
}
