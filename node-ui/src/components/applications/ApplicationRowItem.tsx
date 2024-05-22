import React from "react";
import styled from "styled-components";
import { truncateHash } from "../../utils/displayFunctions";
import { Application } from "../../pages/Applications";

interface ApplicationRowItemProps {
  hasBorders: boolean;
}

const RowItem = styled.div<ApplicationRowItemProps>`
  display: flex;
  width: 100%;
  align-items: center;
  gap: 1px;
  font-size: 0.875rem;
  font-optical-sizing: auto;
  font-weight: 500;
  font-style: normal;
  font-variation-settings: "slnt" 0;
  -webkit-font-smoothing: antialiased;
  -moz-osx-font-smoothing: grayscale;
  font-smooth: never;
  line-height: 1.25rem;
  text-align: left;
  padding-right: 1.5rem;
  padding-left: 1.5rem;
  border-top: 1px solid #23262d;
  ${(props) => props.hasBorders && `border-bottom: 1px solid #23262D;`}

  .row-item {
    padding: 0.75rem 0rem;
    height: 4.5rem;
    display: flex;
    align-items: center;
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

export default function applicationRowItem(
  item: Application,
  id: number,
  count: number,
  onRowItemClick?: (id: string) => void
): JSX.Element {
  return (
    <RowItem key={item.id} hasBorders={id === count}>
      <div
        className="row-item name"
        onClick={() => onRowItemClick && onRowItemClick(item.id)}
      >
        {item.name}
      </div>
      <div className="row-item read">{truncateHash(item.id)}</div>
      <div className="row-item read">{item.version}</div>
      <div className="row-item read">-</div>
    </RowItem>
  );
}
