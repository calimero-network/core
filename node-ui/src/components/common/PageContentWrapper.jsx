import React from "react";
import styled from "styled-components";
import PropTypes from "prop-types";

const PageWrapper = styled.div`
  padding: 4.705rem 2rem 2rem;
  display: flex;
  flex: 1;
  justify-content: center;
  align-items: center;
`;

export default function PageContentWrapper({ children }) {
  return <PageWrapper>{children}</PageWrapper>;
}

PageContentWrapper.propTypes = {
  children: PropTypes.node.isRequired,
};
