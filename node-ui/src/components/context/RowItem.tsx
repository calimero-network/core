import React from "react";
import styled from "styled-components";
import MenuIconDropdown from "../common/MenuIconDropdown";

interface RowItemComponentProps {
  hasBorders: boolean;
}

const RowItem = styled.div<RowItemComponentProps>`
  display: flex;
  align-items: center;
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
    width: 50%;
    padding: 0.75rem 0rem;
    min-height: 3.5rem;
  }

  .id {
    display: flex;
    jusitify-content: center;
    align-items: center;
    padding-left: 1.5rem;
    font-size: 0.875rem
    font-weight: 500;
    line-height: 1.25rem;
    text-align: left;
    color: #fff;
    text-decoration: none;
    word-break: break-word;

    &:hover {
      color: #76f5f9;
    }
  }

  .name {
    color: #6B7280;
    display: flex;
    jusitify-content: center;
    align-items: center;
    padding-left: 1rem;
  }

  .menu-dropdown {
    margin-right: 1rem;
  }
`;


export default function rowItem(item: any, id: number, count: number, onitemClicked: (id: number) => void){
  return (
    <RowItem key={item.id} hasBorders={id === count}>
      <a href={`/admin/contexts/${item.id}`} className="row-item id">
        {item.id}
      </a>
      <div className="row-item name">{item.name}</div>
      <div className="menu-dropdown">
        <MenuIconDropdown
          options={[
            {
              title: "Delete Context",
              onClick: () => onitemClicked(item.id),
            },
          ]}
        />
      </div>
    </RowItem>
  );
}
