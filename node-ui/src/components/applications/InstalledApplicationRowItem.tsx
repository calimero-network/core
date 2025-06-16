import React from 'react';
import styled from 'styled-components';
import { truncateHash } from '../../utils/displayFunctions';
import { Application } from '../../pages/Applications';
import { ClipboardDocumentIcon } from '@heroicons/react/24/solid';
import MenuIconDropdown from '../common/MenuIconDropdown';

interface ApplicationRowItemProps {
  $hasBorders: boolean;
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
    height: 4.5rem;
    display: flex;
    align-items: center;
    width: 20%;
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

  .copy-icon {
    height: 1.5rem;
    width: 1.5rem;
    color: #9c9da3;
    cursor: pointer;
  }
  .copy-icon:hover {
    color: #fff;
  }

  .menu-dropdown {
    padding: 0.75rem 0rem;
    height: 4.5rem;
    display: flex;
    justify-content: end;
    align-items: center;
    width: 20%;
  }
`;

export default function installedApplicationRowItem(
  item: Application,
  id: number,
  count: number,
  onRowItemClick?: (id: string) => void,
): JSX.Element {
  const copyToClippboard = (text: string) => {
    navigator.clipboard.writeText(text).catch((err) => {
      console.error('Failed to copy text to clipboard: ', err);
    });
  };

  return (
    <RowItem key={item.id} $hasBorders={id === count}>
      <div
        className="row-item name"
        onClick={() => onRowItemClick && onRowItemClick(item.id)}
      >
        {item.name}
      </div>
      <div className="row-item read">
        <ClipboardDocumentIcon
          className="copy-icon"
          onClick={() => copyToClippboard(item.id)}
        />
        <span>{truncateHash(item.id)}</span>
      </div>
      <div className="row-item read">{item.version ?? 'N/A'}</div>
      <div className="row-item read">{item.owner ?? 'N/A'}</div>
      <div className="menu-dropdown">
        <MenuIconDropdown
          options={[
            {
              title: 'Uninstall application',
              onClick: () => onRowItemClick && onRowItemClick(item.id),
            },
          ]}
        />
      </div>
    </RowItem>
  );
}
