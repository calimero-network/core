import React from "react";
import styled from "styled-components";

interface HeaderGridProps {
  $optionsCount: number;
}

const HeaderGrid = styled.div<HeaderGridProps>`
  width: 210px;
  display: grid;
  grid-template-columns: repeat(${(props) => props.$optionsCount}, 1fr);
  gap: 1rem;
  padding: 0.75rem 1.5rem;

  .header-option {
    font-size: 0.75rem;
    line-height: 1rem;
    text-align: center;
    color: #fff;
    cursor: pointer;
    width: fit-content;
    white-space: nowrap;
  }

  .active {
    color: #4cfafc;
  }
`;

interface OptionsHeaderProps {
  tableOptions: any[];
  currentOption: string;
  setCurrentOption: (option: string) => void;
  showOptionsCount: boolean;
}

export default function OptionsHeader({
  tableOptions,
  currentOption,
  setCurrentOption,
  showOptionsCount,
}: OptionsHeaderProps) {
  return (
    <HeaderGrid $optionsCount={tableOptions?.length ?? 0}>
      {tableOptions.map((option, index) => {
        return (
          <div
            className={`header-option ${
              currentOption === option.id ? "active" : ""
            }`}
            key={index}
            onClick={() => setCurrentOption(option.id)}
          >
            {`${option.name} ${option.count !== -1 && showOptionsCount ? `(${option.count})` : ""}`}
          </div>
        );
      })}
    </HeaderGrid>
  );
}
