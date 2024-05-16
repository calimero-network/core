import React from "react";
import { ClientKey } from "../../../pages/ContextDetails";
import styled from "styled-components";

interface ClientKeyRowItemProps {
  hasBorders: boolean;
}

const RowItem = styled.div<ClientKeyRowItemProps>`
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
    props.hasBorders
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
    width: 33.33%;
    color: #fff;
  }

  .pk {
    color: #9c9da3;
  }
`;

export default function clientKeyRowItem(item: ClientKey, id: number, count: number) {
  return (
    <RowItem key={item.publicKey} hasBorders={id === count}>
      <div className="row-item type">{item.type}</div>
      <div className="row-item read">{item.date}</div>
      <div className="row-item pk">{item.publicKey}</div>
    </RowItem>
  );
}
