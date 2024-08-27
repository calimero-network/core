import React from 'react';
import styled from 'styled-components';

interface RowItemProps {
  $hasBorders: boolean;
}

const RowItem = styled.div<RowItemProps>`
  display: flex;
  width: 100%;
  align-items: center;
  gap: 1px;
  font-size: 0.875rem;
  font-optical-sizing: auto;
  font-weight: 500;
  font-style: normal;
  font-variation-settings: 'slnt' 0;
  -webkit-font-smoothing: antialiased;
  -moz-osx-font-smoothing: grayscale;
  font-smooth: never;
  line-height: 1.25rem;
  text-align: left;
  padding-right: 1.5rem;
  border-top: 1px solid #23262d;
  ${(props) => props.$hasBorders && `border-bottom: 1px solid #23262D;`}

  .row-item {
    padding: 0.75rem 0rem;
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
 
`;

interface ListItemProps {
  item: string;
  id: number;
  count: number;
  onRowItemClick?: (id: string) => void;
}

export default function ListItem({
  item,
  id,
  count,
  onRowItemClick,
}: ListItemProps) {
  return (
    <RowItem key={id} $hasBorders={id === count}>
      <div
        className="row-item name"
        onClick={() => onRowItemClick && onRowItemClick(item)}
      >
        {item}
      </div>
    </RowItem>
  );
}
