import React from "react";
import styled from "styled-components";

const AddNewButton = styled.div`
  display: flex;
  color: #fff;
  font-size: 14px;
  gap: 4px;
  cursor: pointer;

  .plus {
    color: rgb(255, 255, 255, 0.7);
  }
`;

interface AddNewItemProps {
  text: string;
  onClick: () => void;
}

export function AddNewItem({ text, onClick }: AddNewItemProps) {
  return (
    <AddNewButton onClick={onClick}>
      <span className="plus">+</span> {text}
    </AddNewButton>
  );
}
