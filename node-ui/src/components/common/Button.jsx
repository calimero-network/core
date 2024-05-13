import React from "react";
import styled from "styled-components";
import PropTypes from "prop-types";
import LoaderSpinner from "./LoaderSpinner";

const ButtonStyled = styled.div`
  background-color: #4cfafc;
  height: 2.375rem;
  width: ${(props) => (props.$btnWidth ? props.$btnWidth : "fit-content")};
  padding: 0.625rem 0.75rem;
  border-radius: 0.5rem;
  color: #000;
  font-family: "Inter", sans-serif;
  font-optical-sizing: auto;
  font-weight: 500;
  font-style: normal;
  font-variation-settings: "slnt" 0;
  -webkit-font-smoothing: antialiased;
  -moz-osx-font-smoothing: grayscale;
  font-smooth: never;
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
