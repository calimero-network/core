import React from 'react';
import styled from 'styled-components';
import { formatTimestampToDate } from '../../../utils/date';
import { ClientKey } from '../../../api/dataSource/NodeDataSource';
import { truncateText } from '../../../utils/displayFunctions';

interface ClientKeyRowItemProps {
  $hasBorders: boolean;
}

const RowItem = styled.div<ClientKeyRowItemProps>`
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
  padding-left: 1.5rem;
  border-top: 1px solid #23262d;
  ${(props) => props.$hasBorders && `border-bottom: 1px solid #23262D;`}

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

export default function clientKeyRowItem(
  item: ClientKey,
  id: number,
  count: number,
): JSX.Element {
  return (
    <RowItem key={item.signingKey} $hasBorders={id === count}>
      <div className="row-item type">{item.wallet.type}</div>
      <div className="row-item read">
        {formatTimestampToDate(item.createdAt)}
      </div>
      <div className="row-item pk">{truncateText(item.signingKey)}</div>
    </RowItem>
  );
}
