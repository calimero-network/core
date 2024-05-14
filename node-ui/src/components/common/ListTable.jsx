import React from "react";
import styled from "styled-components";
import PropTypes from "prop-types";

const ListWrapper = styled.div`
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
    grid-template-columns: repeat(${(props) => props.$columnItems}, 1fr);
    grid-template-rows: auto;
    padding: 0.75rem 1.5rem;
    background-color: #15181f;
    ${(props) =>
      props.$roundedTopList &&
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

const TableHeader = ({ tableHeaderItems }) => {
  return (
    <div className="header-items-grid">
      {tableHeaderItems.map((item, index) => {
        return (
          <div className={`header-item`} key={index}>
            {item}
          </div>
        );
      })}
    </div>
  );
};

TableHeader.propTypes = {
  tableHeaderItems: PropTypes.array.isRequired,
};

export default function ListTable({
  ListDescription,
  ListHeaderItems,
  ListItems,
  rowItem,
  columnItems,
  roundedTopList,
  noItemsText,
  onRowItemClick
}) {
  return (
    <ListWrapper $columnItems={columnItems ?? 0} $roundedTopList={roundedTopList}>
      {ListDescription && (
        <div className="table-description">{ListDescription}</div>
      )}
      {ListHeaderItems?.length > 0 && (
        <TableHeader
          tableHeaderItems={ListHeaderItems}
        />
      )}
      <div className="list-items">
        {ListItems?.map((item, id) => rowItem(item, id, ListItems.length - 1, onRowItemClick))}
        {ListItems?.length === 0 && (
          <div className="no-items-text">{noItemsText}</div>
        )}
      </div>
    </ListWrapper>
  );
}

ListTable.propTypes = {
  ListDescription: PropTypes.string,
  ListHeaderItems: PropTypes.array,
  ListItems: PropTypes.array.isRequired,
  rowItem: PropTypes.func.isRequired,
  columnItems: PropTypes.number.isRequired,
  roundedTopList: PropTypes.bool.isRequired,
  noItemsText: PropTypes.string.isRequired,
  onRowItemClick: PropTypes.func,
};
