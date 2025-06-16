import React from 'react';
import styled from 'styled-components';

interface ListWrapperProps {
  $numOfColumns: number;
  $roundTopItem: boolean;
}

const ListWrapper = styled.div<ListWrapperProps>`
  display: flex;
  flex-direction: column;
  flex: 1;
  width: 100%;
  max-height: 100%;
  overflow: hidden;

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
    grid-template-columns: repeat(${(props) => props.$numOfColumns}, 1fr);
    grid-template-rows: auto;
    padding: 0.75rem 1.5rem;
    background-color: #15181f;
    ${(props) =>
      props.$roundTopItem &&
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
    max-height: 800px;
    overflow-y: auto;
  }

  .container {
    display: flex;
    align-items: center;
    width: 100%;
    padding: 1rem;
    display: flex;
    flex-direction: column;

    .error-text {
      font-weight: 500;
      color: #6b7280;
      padding-bottom: 4px;
    }
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
  error?: string;
  rowItem: (
    item: T,
    id: number,
    lastIndex: number,
    onRowItemClick?: (id: string) => void,
  ) => JSX.Element;
  numOfColumns: number;
  roundTopItem: boolean;
  noItemsText: string;
  onRowItemClick?: (id: string, isAccepted?: boolean) => void;
}

export default function ListTable<T>(props: ListTableProps<T>) {
  return (
    <ListWrapper
      $numOfColumns={props.numOfColumns ?? 0}
      $roundTopItem={props.roundTopItem}
    >
      {props.listDescription && (
        <div className="table-description">{props.listDescription}</div>
      )}
      {props.listHeaderItems && props.listHeaderItems?.length > 0 && (
        <TableHeader tableHeaderItems={props.listHeaderItems} />
      )}
      {props.error ? (
        <div className="container">
          <div className="error-text">{props.error}</div>
        </div>
      ) : (
        <div className="list-items">
          {props.listItems?.map((item: T, id: number) =>
            props.rowItem(
              item,
              id,
              props.listItems.length - 1,
              props?.onRowItemClick,
            ),
          )}
          {props.listItems?.length === 0 && (
            <div className="no-items-text">{props.noItemsText}</div>
          )}
        </div>
      )}
    </ListWrapper>
  );
}
