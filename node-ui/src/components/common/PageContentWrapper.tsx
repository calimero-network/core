import React from "react";
import styled from "styled-components";

const PageWrapper = styled.div`
  padding: 4.705rem 2rem 2rem;
  display: flex;
  flex: 1;
  justify-content: center;
  align-items: center;
`;

interface PageContentWrapperProps {
  children: React.ReactNode;
}

export default function PageContentWrapper({ children }: PageContentWrapperProps) {
  return <PageWrapper>{children}</PageWrapper>;
}
