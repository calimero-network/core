import React from "react";
import styled from "styled-components";
import PropTypes from "prop-types";

const FlexWrapper = styled.div`
  display: flex;
  height: 100vh;
  background-color: #121216;
`;

export function FlexLayout({ children }) {
  return <FlexWrapper>{children}</FlexWrapper>;
}

FlexLayout.propTypes = {
  children: PropTypes.node.isRequired,
};
