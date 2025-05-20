import React from 'react';
import styled from '@emotion/styled';
import { Provider } from '../../types/auth';

const Container = styled.div`
  width: 100%;
  max-width: 500px;
  padding: 20px;
`;

const ProviderTitle = styled.h2`
  margin-bottom: 20px;
  text-align: center;
  color: #333;
`;

const ProviderGrid = styled.div`
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(240px, 1fr));
  gap: 16px;
`;

const ProviderCard = styled.div`
  display: flex;
  flex-direction: column;
  align-items: center;
  padding: 24px 16px;
  border-radius: 8px;
  background-color: #f5f5f5;
  cursor: pointer;
  transition: all 0.2s ease;
  box-shadow: 0 2px 4px rgba(0, 0, 0, 0.1);
  
  &:hover {
    transform: translateY(-2px);
    box-shadow: 0 4px 8px rgba(0, 0, 0, 0.1);
  }

  &:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }
`;

const ProviderIcon = styled.div`
  width: 48px;
  height: 48px;
  display: flex;
  align-items: center;
  justify-content: center;
  margin-bottom: 16px;
  background-color: #eee;
  border-radius: 50%;
  
  svg {
    width: 24px;
    height: 24px;
  }
`;

const ProviderName = styled.div`
  font-weight: 500;
  margin-bottom: 8px;
`;

const ProviderDescription = styled.div`
  font-size: 14px;
  color: #666;
  text-align: center;
`;

const LoadingSpinner = styled.div`
  width: 32px;
  height: 32px;
  margin: 40px auto;
  border: 4px solid rgba(0, 0, 0, 0.1);
  border-radius: 50%;
  border-top-color: #3498db;
  animation: spin 1s ease-in-out infinite;
  
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
`;

const NoProvidersMessage = styled.div`
  text-align: center;
  padding: 32px;
  color: #666;
`;

interface ProviderSelectorProps {
  providers: Provider[];
  onProviderSelect: (provider: Provider) => void;
  loading: boolean;
}

const ProviderSelector: React.FC<ProviderSelectorProps> = ({ 
  providers, 
  onProviderSelect,
  loading
}) => {
  // Icons for providers (could be replaced with actual icons)
  const getProviderIcon = (providerType: string) => {
    switch (providerType) {
      case 'near_wallet':
        return (
          <svg viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">
            <path d="M15.2 2H8.8C7.11984 2 6.27976 2 5.63803 2.32698C5.07354 2.6146 4.6146 3.07354 4.32698 3.63803C4 4.27976 4 5.11984 4 6.8V17.2C4 18.8802 4 19.7202 4.32698 20.362C4.6146 20.9265 5.07354 21.3854 5.63803 21.673C6.27976 22 7.11984 22 8.8 22H15.2C16.8802 22 17.7202 22 18.362 21.673C18.9265 21.3854 19.3854 20.9265 19.673 20.362C20 19.7202 20 18.8802 20 17.2V6.8C20 5.11984 20 4.27976 19.673 3.63803C19.3854 3.07354 18.9265 2.6146 18.362 2.32698C17.7202 2 16.8802 2 15.2 2Z" stroke="#5D8EF9" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"/>
            <path d="M9 2V22" stroke="#5D8EF9" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"/>
          </svg>
        );
      default:
        return (
          <svg viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">
            <path d="M16 7C16 9.20914 14.2091 11 12 11C9.79086 11 8 9.20914 8 7C8 4.79086 9.79086 3 12 3C14.2091 3 16 4.79086 16 7Z" stroke="#5D8EF9" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"/>
            <path d="M12 14C8.13401 14 5 17.134 5 21H19C19 17.134 15.866 14 12 14Z" stroke="#5D8EF9" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round"/>
          </svg>
        );
    }
  };

  if (loading) {
    return <LoadingSpinner />;
  }
  
  if (providers.length === 0) {
    return (
      <NoProvidersMessage>
        No authentication providers are available.
      </NoProvidersMessage>
    );
  }
  
  return (
      <Container>
      <ProviderTitle>Choose an authentication method</ProviderTitle>
      <ProviderGrid>
        {providers.map((provider) => (
          <ProviderCard
              key={provider.name}
            onClick={() => onProviderSelect(provider)}
          >
            <ProviderIcon>
              {getProviderIcon(provider.name)}
            </ProviderIcon>
            <ProviderName>{provider.name}</ProviderName>
            <ProviderDescription>
              {provider.description}
            </ProviderDescription>
          </ProviderCard>
        ))}
      </ProviderGrid>
      </Container>
  );
};

export default ProviderSelector;