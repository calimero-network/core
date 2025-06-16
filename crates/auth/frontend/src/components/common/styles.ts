import styled from 'styled-components';

export const LoadingWrapper = styled.div`
  text-align: center;
  padding: ${({ theme }) => theme.spacing.xxl};
  color: ${({ theme }) => theme.colors.text.secondary};
  background: ${({ theme }) => theme.colors.background.primary};
  border-radius: ${({ theme }) => theme.borderRadius.lg};
  box-shadow: ${({ theme }) => theme.shadows.lg};
  position: fixed;
  top: 50%;
  left: 50%;
  transform: translate(-50%, -50%);
  z-index: ${({ theme }) => theme.zIndex.modal};
  min-width: 200px;

  &::before {
    content: '';
    position: fixed;
    top: 0;
    left: 0;
    right: 0;
    bottom: 0;
    background: rgba(0, 0, 0, 0.5);
    z-index: -1;
  }
`;

export const EmptyState = styled.div`
  text-align: center;
  padding: ${({ theme }) => theme.spacing.xxl};
  background-color: ${({ theme }) => theme.colors.background.secondary};
  border-radius: ${({ theme }) => theme.borderRadius.default};
  border: 1px dashed ${({ theme }) => theme.colors.border.primary};

  h2 {
    color: ${({ theme }) => theme.colors.text.primary};
    margin: 0 0 ${({ theme }) => theme.spacing.md} 0;
    font-size: ${({ theme }) => theme.typography.title.size};
  }

  p {
    color: ${({ theme }) => theme.colors.text.secondary};
    margin: 0;
    line-height: 1.5;
    max-width: 400px;
    margin: 0 auto;
    font-size: ${({ theme }) => theme.typography.body.size};
  }
`;

export const ButtonGroup = styled.div`
  display: flex;
  gap: ${({ theme }) => theme.spacing.md};
  justify-content: center;
  margin-top: ${({ theme }) => theme.spacing.lg};
`;

export const ErrorContainer = styled.div`
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: ${({ theme }) => theme.spacing.lg};
  padding: ${({ theme }) => theme.spacing.xl};
  text-align: center;
  margin: auto;
  background-color: ${({ theme }) => theme.colors.background.secondary};
`;

export const ErrorMessage = styled.div`
  color: ${({ theme }) => theme.colors.text.error};
  font-size: ${({ theme }) => theme.typography.body.size};
  margin-bottom: ${({ theme }) => theme.spacing.md};
`;