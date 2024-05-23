import React from "react";
import styled from "styled-components";

interface ListWrapperProps {
  numOfColumns: number;
  roundTopItem: boolean;
}

const ListWrapper = styled.div<ListWrapperProps>`
  display: flex;
  flex-direction: column;
  flex: 1;
  width: 100%;
  max-height: calc(100vh - 18rem);

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
    grid-template-columns: repeat(${(props) => props.numOfColumns}, 1fr);
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

  .list-items {
    overflow-y: scroll;
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
  listDescription?: string;
  listHeaderItems?: string[];
  listItems: T[];
  rowItem: (
    item: T,
    id: number,
    lastIndex: number,
    onRowItemClick?: (id: string) => void
  ) => JSX.Element;
  numOfColumns: number;
  roundTopItem: boolean;
  noItemsText: string;
  onRowItemClick?: (id: string) => void;
}

export default function ListTable<T>({
  listDescription,
  listHeaderItems,
  listItems,
  rowItem,
  numOfColumns,
  roundTopItem,
  noItemsText,
  onRowItemClick,
}: ListTableProps<T>) {
  return (
    <ListWrapper numOfColumns={numOfColumns ?? 0} roundTopItem={roundTopItem}>
      {listDescription && (
        <div className="table-description">{listDescription}</div>
      )}
      {listHeaderItems && listHeaderItems?.length > 0 && (
        <TableHeader tableHeaderItems={listHeaderItems} />
      )}
      <div className="list-items">
        {listItems?.map((item: T, id: number) =>
          rowItem(item, id, listItems.length - 1, onRowItemClick)
        )}
        {listItems?.length === 0 && (
          <div className="no-items-text">{noItemsText}</div>
        )}
      </div>
    </ListWrapper>
  );
}
