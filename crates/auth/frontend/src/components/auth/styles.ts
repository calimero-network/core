import styled from 'styled-components';

export const Container = styled.div`
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  width: 100%;
  padding: ${({ theme }) => theme.spacing.xl};
`;

export const ErrorMessage = styled.div`
  color: ${({ theme }) => theme.colors.text.error};
  margin-bottom: ${({ theme }) => theme.spacing.lg};
  text-align: center;
`;