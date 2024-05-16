import React from "react";
import styled from "styled-components";
import MenuIconDropdown from "../common/MenuIconDropdown";
import { RootKeyObject } from "../../utils/rootkey";

interface RowItemComponentProps {
  $borders: boolean;
}

const RowItem = styled.div<RowItemComponentProps>`
  display: flex;
  align-items: center;
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
    width: fit-content;
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

export default function IdentityRowItem(
  item: RootKeyObject,
  id: number,
  count: number,
  onitemClicked: (id: string) => void
) {
  return (
    <RowItem key={id} $borders={id === count}>
      <div className="row-item type">{item.type}</div>
      <div className="row-item date">{item.date}</div>
      <div className="row-item public-key">{item.publicKey}</div>
      <div className="menu-dropdown">
        <MenuIconDropdown
          options={[
            {
              title: "Copy key",
              onClick: () => onitemClicked(item.publicKey),
            },
          ]}
        />
      </div>
    </RowItem>
  );
}
