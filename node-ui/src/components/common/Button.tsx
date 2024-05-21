import React from "react";
import styled from "styled-components";
import LoaderSpinner from "./LoaderSpinner";

interface StyledButtonProps {
  btnWidth?: string;
}

const ButtonStyled = styled.button<StyledButtonProps>`
  background-color: ${(props) => (props.disabled ? "#434546" : "#4cfafc")};
  height: 2.375rem;
  width: ${(props) => (props.btnWidth ? props.btnWidth : "fit-content")};
  padding: 0.625rem 0.75rem;
  border-radius: 0.5rem;
  color: #000;
  font-size: 0.875rem;
  font-weight: 500;
  line-height: 1.25rem;
  text-align: center;
  cursor: pointer;
  outline: none;
  border: none;

  &:hover {
    background-color: #76f5f9;
  }
`;

interface ButtonProps {
  onClick: () => void;
  text: string;
  width?: string;
  isLoading?: boolean;
  isDisabled?: boolean;
}

export default function Button({
  onClick,
  text,
  width,
  isLoading,
  isDisabled = false,
}: ButtonProps) {
  return (
    <ButtonStyled
      onClick={onClick}
      btnWidth={width ?? ""}
      disabled={isDisabled}
    >
      {isLoading ? <LoaderSpinner /> : text}
    </ButtonStyled>
  );
}
