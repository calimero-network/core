import React from 'react';
import styled from '@emotion/styled';

interface CardProps {
  title?: string;
  children: React.ReactNode;
}

const CardContainer = styled.div`
  background: white;
  border-radius: 8px;
  box-shadow: 0 2px 10px rgba(0, 0, 0, 0.1);
  padding: 24px;
  margin-bottom: 20px;
`;

const CardTitle = styled.h2`
  font-size: 20px;
  margin-top: 0;
  margin-bottom: 16px;
`;

const Card: React.FC<CardProps> = ({ title, children }) => {
  return (
    <CardContainer>
      {title && <CardTitle>{title}</CardTitle>}
      {children}
    </CardContainer>
  );
};

export default Card;