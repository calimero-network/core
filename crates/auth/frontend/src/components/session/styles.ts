import styled from 'styled-components';

export const SessionPromptContainer = styled.div`
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: ${({ theme }) => theme.spacing.lg};
  text-align: center;
  padding: ${({ theme }) => theme.spacing.xl};
  border: 1px solid ${({ theme }) => theme.colors.border.primary};
  height: fit-content;
`;

export const Title = styled.h2`
  color: ${({ theme }) => theme.colors.text.primary};
  margin: 0;
`;

export const Description = styled.p`
  color: ${({ theme }) => theme.colors.text.secondary};
  margin: ${({ theme }) => theme.spacing.md} 0;
`;

export const ButtonGroup = styled.div`
  display: flex;
  gap: ${({ theme }) => theme.spacing.md};
  margin-top: ${({ theme }) => theme.spacing.lg};
  width: 100%;
`;