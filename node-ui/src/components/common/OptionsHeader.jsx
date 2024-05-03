import React from "react";
import styled from "styled-components";
import PropTypes from "prop-types";

const HeaderGrid = styled.div`
  font-family: Inter;
  width: fit-content;
  display: grid;
  grid-template-columns: repeat(${(props) => props.optionsCount}, 1fr);
  grid-template-rows: auto;
  gap: 1rem;
  padding: 0.75rem 1.5rem;

  .header-option {
    font-size: 0.75rem;
    font-weight: 500;
    line-height: 1rem;
    text-align: left;
    color: #fff;
    grid-column: span 1;
    cursor: pointer;
  }

  .active {
    color: #4cfafc;
  }
`;

export default function OptionsHeader({
  tableOptions,
  currentOption,
  setCurrentOption,
  showOptionsCount,
}) {
  return (
    <HeaderGrid optionsCount={tableOptions?.length ?? 0}>
      {tableOptions.map((option, index) => {
        return (
          <div
            className={`header-option ${
              currentOption === option.id ? "active" : ""
            }`}
            key={index}
            onClick={() => setCurrentOption(option.id)}
          >
            {`${option.name} ${showOptionsCount ? `(${option.count})` : ""}`}
          </div>
        );
      })}
    </HeaderGrid>
  );
}

OptionsHeader.propTypes = {
  tableOptions: PropTypes.array.isRequired,
  currentOption: PropTypes.string,
  setCurrentOption: PropTypes.func,
  showOptionsCount: PropTypes.bool,
};
