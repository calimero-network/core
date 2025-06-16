import React from 'react';
import styled from 'styled-components';
import MenuIconDropdown from '../common/MenuIconDropdown';
import { formatTimestampToDate } from '../../utils/date';
import { RootKey } from '@calimero-network/calimero-client/lib/api/adminApi';

interface RowItemProps {
  $hasBorders: boolean;
}

const RowItem = styled.div<RowItemProps>`
  display: flex;
  align-items: center;
  padding-left: 1.5rem;
  ${(props) =>
    props.$hasBorders
      ? `
    border-top: 1px solid #23262D;
    border-bottom: 1px solid #23262D;
  `
      : `
    border-top: 1px solid #23262D;
  `}

  .type, .date {
    color: #fff;
  }
  .row-item {
    width: 16%;
    min-width: 4.375rem;
    padding: 0.75rem 0rem;
    min-height: 3.5rem;
    display: flex;
    jusitify-content: center;
    align-items: center;
    font-size: 0.875rem;
    line-height: 1.25rem;
  }

  .public-key {
    color: #6b7280;
    width: 64%;
    word-break: break-all;
  }

  .menu-dropdown {
    flex: 1;
    display: flex;
    justify-content: flex-end;
    align-items: center;
    margin-right: 1rem;
    padding-bottom: 4px;
  }
`;

export default function identityRowItem(
  item: RootKey,
  id: number,
  count: number,
  onCopyClick?: (id: string) => void,
  onEditClick?: (id: string) => void,
  onDeleteClick?: (id: string) => void,
): JSX.Element {
  return (
    <RowItem key={id} $hasBorders={id === count}>
      <div className="row-item type">{item.auth_method}</div>
      <div className="row-item date">
        {formatTimestampToDate(item.created_at)}
      </div>
      <div className="row-item public-key">{item.public_key}</div>
      <div className="menu-dropdown">
        <MenuIconDropdown
          options={[
            {
              title: 'Copy key',
              onClick: () => onCopyClick && onCopyClick(item.public_key),
            },
            {
              title: 'Edit Permissions',
              onClick: () => onEditClick && onEditClick(item.public_key),
            },
            {
              title: 'Delete Key',
              onClick: () => onDeleteClick && onDeleteClick(item.key_id),
            },
          ]}
        />
      </div>
    </RowItem>
  );
}
