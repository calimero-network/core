import styled from 'styled-components';

export const PermissionsContainer = styled.div`
  display: flex;
  flex-direction: column;
  gap: ${({ theme }) => theme.spacing.lg};
  padding: ${({ theme }) => theme.spacing.xl};
`;

export const PermissionsInfo = styled.div`
  margin-top: ${({ theme }) => theme.spacing.xl};
  padding: ${({ theme }) => theme.spacing.lg};
  border-radius: ${({ theme }) => theme.borderRadius.default};
  background-color: ${({ theme }) => theme.colors.background.secondary};
  border: 1px solid ${({ theme }) => theme.colors.border.primary};

  h3 {
    margin: 0 0 ${({ theme }) => theme.spacing.md} 0;
    color: ${({ theme }) => theme.colors.text.primary};
    font-size: ${({ theme }) => theme.typography.subtitle.size};
  }
`;

export const PermissionsList = styled.ul`
  list-style: none;
  padding: 0;
  margin: ${({ theme }) => theme.spacing.md} 0;

  li {
    padding: ${({ theme }) => `${theme.spacing.sm} ${theme.spacing.md}`};
    margin: ${({ theme }) => theme.spacing.xs} 0;
    background-color: ${({ theme }) => theme.colors.background.tertiary};
    border-radius: ${({ theme }) => theme.borderRadius.sm};
    font-family: monospace;
    color: ${({ theme }) => theme.colors.text.secondary};
  }
`;

export const PermissionsNotice = styled.p`
  margin: ${({ theme }) => theme.spacing.md} 0 0 0;
  color: ${({ theme }) => theme.colors.text.secondary};
  font-size: ${({ theme }) => theme.typography.small.size};
  font-style: italic;
`;

export const ButtonGroup = styled.div`
  display: flex;
  gap: ${({ theme }) => theme.spacing.md};
  justify-content: center;
  margin-top: ${({ theme }) => theme.spacing.lg};
`;
