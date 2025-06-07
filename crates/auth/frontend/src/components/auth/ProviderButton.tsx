import React from 'react';
import styled from '@emotion/styled';
import { Provider } from '../../types/auth';

interface ProviderButtonProps {
  provider: Provider;
  selected: boolean;
  onClick: () => void;
}

const Button = styled.button<{ selected: boolean }>`
  display: flex;
  align-items: center;
  width: 100%;
  padding: 12px 16px;
  border-radius: 4px;
  background-color: ${props => props.selected ? '#e6f7ff' : 'white'};
  border: 1px solid ${props => props.selected ? 'var(--primary-color)' : 'var(--border-color)'};
  cursor: pointer;
  transition: all 0.2s;
  margin-bottom: 12px;
  text-align: left;
  
  &:hover {
    background-color: #f5f9ff;
    border-color: var(--primary-color);
  }
`;

const Icon = styled.div<{ providerType: string }>`
  width: 24px;
  height: 24px;
  margin-right: 16px;
  background-size: contain;
  background-position: center;
  background-repeat: no-repeat;
`;

const Info = styled.div`
  flex: 1;
`;

const Name = styled.div`
  font-weight: 500;
  color: #333;
  margin-bottom: 4px;
`;

const Description = styled.div`
  font-size: 14px;
  color: #666;
`;

const ProviderButton: React.FC<ProviderButtonProps> = ({ provider, selected, onClick }) => {
  return (
    <Button selected={selected} onClick={onClick}>
      <Icon providerType={provider.name} />
      <Info>
        <Name>{provider.name}</Name>
        <Description>{provider.description}</Description>
      </Info>
    </Button>
  );
};

export default ProviderButton;