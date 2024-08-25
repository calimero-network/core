import React from 'react';
import styled from 'styled-components';
import { truncateHash } from '../../../utils/displayFunctions';
import { Release } from '../../../pages/Applications';
import { ClipboardDocumentIcon } from '@heroicons/react/24/solid';

export interface RowItemProps {
  $hasBorders: boolean;
}

export const RowItem = styled.div<RowItemProps>`
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
    width: 25%;
  }

  .read {
    color: #9c9da3;
  }

  .long-text {
    width: 90%;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
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
`;

export default function releaseRowItem(
  item: Release,
  id: number,
  count: number,
): JSX.Element {
  const copyToClippboard = (text: string) => {
    navigator.clipboard.writeText(text).catch((err) => {
      console.error('Failed to copy text to clipboard: ', err);
    });
  };

  return (
    <RowItem key={item.hash} $hasBorders={id === count}>
      <div className="row-item name">{item.version}</div>
      <div className="row-item read">
        <ClipboardDocumentIcon
          className="copy-icon"
          onClick={() => copyToClippboard(item.path)}
        />
        <span className="long-text">{item.path.substring(0, 20)}...</span>
      </div>
      <div className="row-item read">
        <span className="long-text">{item.notes}</span>
      </div>
      <ClipboardDocumentIcon
        className="copy-icon"
        onClick={() => copyToClippboard(item.hash)}
      />
      <div className="row-item read">{truncateHash(item.hash)}</div>
    </RowItem>
  );
}
