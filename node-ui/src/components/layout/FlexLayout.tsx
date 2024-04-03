import React from "react";
import styled from "styled-components";

const FlexWrapper = styled.div`
  display: flex;
  height: 100vh;
`;

export function FlexLayout({ children }) {
  return <FlexWrapper>{children}</FlexWrapper>;
}
