import React from 'react';
import styled from 'styled-components';

const PageWrapper = styled.div<{ $isOverflow: boolean }>`
  padding: 4.705rem 2rem 2rem;
  ${(props) => (props.$isOverflow ? 'overflow-y: scroll;' : 'display: flex;')}
  flex: 1;
  // justify-content: center;
  // align-items: center;
`;

interface PageContentWrapperProps {
  children: React.ReactNode;
  isOverflow?: boolean;
}

export default function PageContentWrapper({
  children,
  isOverflow = false,
}: PageContentWrapperProps) {
  return <PageWrapper $isOverflow={isOverflow}>{children}</PageWrapper>;
}
