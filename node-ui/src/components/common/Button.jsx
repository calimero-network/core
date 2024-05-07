import React from "react";
import styled from "styled-components";
import PropTypes from "prop-types";
import LoaderSpinner from "./LoaderSpinner";

const ButtonStyled = styled.div`
  background-color: #4cfafc;
  height: 2.375rem;
  width: ${(props) => (props.$btnWidth ? props.$btnWidth : "fit-content")};
  padding: 9px 12px;
  border-radius: 0.5rem;
  color: #000;
  font-family: Inter;
  font-size: 0.875rem;
  font-weight: 500;
  line-height: 1.25rem;
  text-align: center;
  cursor: pointer;

  &:hover {
    background-color: #76f5f9;
  }
`;

export default function Button({ onClick, text, width, isLoading }) {
  return (
    <ButtonStyled onClick={onClick} $btnWidth={width}>
      {isLoading ? <LoaderSpinner/> : text}
    </ButtonStyled>
  );
}

Button.propTypes = {
  onClick: PropTypes.func.isRequired,
  text: PropTypes.string.isRequired,
  width: PropTypes.string,
  isLoading: PropTypes.bool,
};
