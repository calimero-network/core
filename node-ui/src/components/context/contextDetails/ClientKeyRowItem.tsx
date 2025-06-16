import React from 'react';
import styled from 'styled-components';
import { formatTimestampToDate } from '../../../utils/date';
// import { ClientKey } from '../../../types/client-key';
import { truncateText } from '../../../utils/displayFunctions';
import MenuIconDropdown from '../../common/MenuIconDropdown';

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
    width: 25%;
    color: #fff;
  }

  .pk {
    color: #9c9da3;
    width: 40%;
  }

  .menu-dropdown {
    width: 10%;
    display: flex;
    justify-content: flex-end;
    align-items: center;
  }
`;

export default function clientKeyRowItem(
  item: ClientKey,
  id: number,
  count: number,
  onCopyClick?: (id: string) => void,
  onRevokeClick?: (id: string) => void,
): JSX.Element {
  const menuOptions = [];
  
  if (onCopyClick) {
    menuOptions.push({
      title: 'Copy ID',
      onClick: () => onCopyClick(item.client_id),
    });
  }

  if (!item.revoked_at && onRevokeClick) {
    menuOptions.push({
      title: 'Revoke Key',
      onClick: () => onRevokeClick(item.client_id),
    });
  }

  return (
    <RowItem key={item.client_id} $hasBorders={id === count}>
      <div className="row-item type">{item.name}</div>
      <div className="row-item">
        {formatTimestampToDate(item.created_at)}
      </div>
      <div className="row-item pk">{truncateText(item.client_id)}</div>
      {menuOptions.length > 0 && (
        <div className="menu-dropdown">
          <MenuIconDropdown options={menuOptions} />
        </div>
      )}
    </RowItem>
  );
}
