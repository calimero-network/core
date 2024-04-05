import React from "react";
import styled from "styled-components";
import PropTypes from "prop-types";

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

export function AddNewItem({ text, onClick }) {
  return (
    <AddNewButton onClick={onClick}>
      <span className="plus">+</span> {text}
    </AddNewButton>
  );
}

AddNewItem.propTypes = {
  text: PropTypes.string.isRequired,
  onClick: PropTypes.func.isRequired,
};
