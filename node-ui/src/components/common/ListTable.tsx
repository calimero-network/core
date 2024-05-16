import React from "react";
import styled from "styled-components";

interface ListWrapperProps {
  columnItems: number;
  roundTopItem: boolean;
}

const ListWrapper = styled.div<ListWrapperProps>`
  display: flex;
  flex-direction: column;
  flex: 1;
  width: 100%;

  .table-description {
    padding-left: 1rem;
    margin-top: 0.375rem;
    margin-bottom: 1rem;
    font-size: 0.75rem;
    font-weight: 400;
    line-height: 1.25rem;
    text-align: left;
    color: #9c9da3;
  }

  .header-items-grid {
    display: grid;
    grid-template-columns: repeat(${(props) => props.columnItems}, 1fr);
    grid-template-rows: auto;
    padding: 0.75rem 1.5rem;
    background-color: #15181f;
    ${(props) =>
      props.roundTopItem &&
      `
        border-top-left-radius: 0.5rem;
        border-top-right-radius: 0.5rem;
      `}

    .header-item {
      font-size: 0.75rem;
      font-weight: 500;
      line-height: 1rem;
      text-align: left;
      letter-spacing: 0.05em;
      color: #6b7280;
      grid-column: span 1;
      cursor: pointer;
    }
  }

  .no-items-text {
    padding: 1rem;
    font-size: 0.75rem;
    font-weight: 400;
    line-height: 1rem;
    text-align: center;
    color: #9c9da3;
  }
`;

interface TableHeaderProps {
  tableHeaderItems: string[];
}

const TableHeader = ({ tableHeaderItems }: TableHeaderProps) => {
  return (
    <div className="header-items-grid">
      {tableHeaderItems.map((item: string, index: number) => {
        return (
          <div className={`header-item`} key={index}>
            {item}
          </div>
        );
      })}
    </div>
  );
};

interface ListTableProps<T> {
  ListDescription?: string;
  ListHeaderItems?: string[];
  ListItems: T[];
  rowItem: (item: T, id: number, lastIndex: number, onRowItemClick: any) => JSX.Element;
  columnItems: number;
  roundTopItem: boolean;
  noItemsText: string;
  onRowItemClick?: (id: string) => void;
}

export default function ListTable<T>({
  ListDescription,
  ListHeaderItems,
  ListItems,
  rowItem,
  columnItems,
  roundTopItem,
  noItemsText,
  onRowItemClick
}: ListTableProps<T>) {
  return (
    <ListWrapper columnItems={columnItems ?? 0} roundTopItem={roundTopItem}>
      {ListDescription && (
        <div className="table-description">{ListDescription}</div>
      )}
      {ListHeaderItems && ListHeaderItems?.length > 0 && (
        <TableHeader
          tableHeaderItems={ListHeaderItems}
        />
      )}
      <div className="list-items">
        {ListItems?.map((item: T, id: number) => rowItem(item, id, ListItems.length - 1, onRowItemClick))}
        {ListItems?.length === 0 && (
          <div className="no-items-text">{noItemsText}</div>
        )}
      </div>
    </ListWrapper>
  );
}
