import React from "react";
import styled from "styled-components";
import PropTypes from "prop-types";

const PageWrapper = styled.div`
  padding: 2rem;
  display: flex;
  justify-content: center;
  align-items: center;
`;

export default function PageContentWrapper({ children }) {
  return <PageWrapper>{children}</PageWrapper>;
}

PageContentWrapper.propTypes = {
  children: PropTypes.node.isRequired,
};
