import styled from 'styled-components';

export const ProviderContainer = styled.div`
  width: 100%;
  padding: ${({ theme }) => theme.spacing.lg};
  margin: 0 auto;
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
`;

export const ProviderTitle = styled.h2`
  margin-bottom: ${({ theme }) => theme.spacing.lg};
  text-align: center;
  color: ${({ theme }) => theme.colors.text.primary};
  font-size: ${({ theme }) => theme.typography.title.size};
  font-weight: ${({ theme }) => theme.typography.title.weight};
`;

export const ProviderGrid = styled.div`
  display: grid;
  gap: ${({ theme }) => theme.spacing.md};
`;

export const ProviderCard = styled.div`
  display: flex;
  flex-direction: column;
  align-items: center;
  padding: ${({ theme }) => `${theme.spacing.xl} ${theme.spacing.lg}`};
  border-radius: ${({ theme }) => theme.borderRadius.default};
  background-color: ${({ theme }) => theme.colors.background.secondary};
  cursor: pointer;
  transition: ${({ theme }) => theme.transitions.default};
  box-shadow: ${({ theme }) => theme.shadows.sm};
  
  &:hover {
    transform: translateY(-2px);
    box-shadow: ${({ theme }) => theme.shadows.default};
  }

  &:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }
`;

export const ProviderIcon = styled.div`
  width: 48px;
  height: 48px;
  display: flex;
  align-items: center;
  justify-content: center;
  margin-bottom: ${({ theme }) => theme.spacing.lg};
  background-color: ${({ theme }) => theme.colors.background.tertiary};
  border-radius: 50%;
  
  svg {
    width: 24px;
    height: 24px;
    
    path {
      stroke: ${({ theme }) => theme.colors.accent.primary};
    }
  }
`;

export const ProviderName = styled.div`
  font-weight: ${({ theme }) => theme.typography.subtitle.weight};
  margin-bottom: ${({ theme }) => theme.spacing.sm};
  color: ${({ theme }) => theme.colors.text.primary};
`;

export const ProviderDescription = styled.div`
  font-size: ${({ theme }) => theme.typography.body.size};
  color: ${({ theme }) => theme.colors.text.secondary};
  text-align: center;
`;

export const LoadingSpinner = styled.div`
  width: 32px;
  height: 32px;
  margin: ${({ theme }) => theme.spacing.xxl} auto;
  border: 4px solid ${({ theme }) => theme.colors.border.primary};
  border-radius: 50%;
  border-top-color: ${({ theme }) => theme.colors.accent.primary};
  animation: spin 1s ease-in-out infinite;
  
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
`;

export const NoProvidersMessage = styled.div`
  text-align: center;
  padding: ${({ theme }) => theme.spacing.xl};
  color: ${({ theme }) => theme.colors.text.secondary};
`;

export const ButtonContainer = styled.div`
  margin-top: ${({ theme }) => theme.spacing.xl};
  width: 100%;
  display: flex;
  justify-content: center;
`; 