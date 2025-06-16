import styled from 'styled-components';

export const ContextSelectorWrapper = styled.div`
  position: fixed;
  top: 50%;
  left: 50%;
  transform: translate(-50%, -50%);
  padding: ${({ theme }) => theme.spacing.xl};
  margin: 0;
  background: ${({ theme }) => theme.colors.background.primary};
  border-radius: ${({ theme }) => theme.borderRadius.lg};
  z-index: ${({ theme }) => theme.zIndex.modal};


  p {
    text-align: center;
    margin-bottom: ${({ theme }) => theme.spacing.lg};
  }
`;

export const ContextList = styled.div`
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(300px, 1fr));
  gap: ${({ theme }) => theme.spacing.lg};
  margin: ${({ theme }) => theme.spacing.lg} 0;
`;

export const ContextItem = styled.div<{ selected?: boolean }>`
  padding: ${({ theme }) => theme.spacing.lg};
  border: 1px solid ${({ theme, selected }) => 
    selected ? theme.colors.accent.primary : theme.colors.border.primary};
  border-radius: ${({ theme }) => theme.borderRadius.default};
  cursor: pointer;
  transition: ${({ theme }) => theme.transitions.default};
  word-break: break-all;
  background-color: ${({ theme, selected }) => 
    selected ? theme.colors.background.secondary : 'transparent'};

  &:hover {
    border-color: ${({ theme }) => theme.colors.accent.primary};
  }

  h3 {
    margin: 0 0 ${({ theme }) => theme.spacing.md} 0;
    color: ${({ theme }) => theme.colors.text.primary};
    font-size: ${({ theme }) => theme.typography.subtitle.size};
    font-weight: ${({ theme }) => theme.typography.subtitle.weight};
  }

  p {
    margin: 0 0 ${({ theme }) => theme.spacing.sm} 0;
    color: ${({ theme }) => theme.colors.text.secondary};
    font-size: ${({ theme }) => theme.typography.body.size};
    line-height: ${({ theme }) => theme.typography.body.lineHeight};
  }
`;

export const IdentityList = styled(ContextList)``;

export const IdentityItem = styled(ContextItem)`
  text-align: center;

  h4 {
    color: ${({ theme }) => theme.colors.text.secondary};
    text-transform: uppercase;
    letter-spacing: 0.5px;
    font-size: ${({ theme }) => theme.typography.small.size};
    margin-bottom: ${({ theme }) => theme.spacing.sm};
  }

  .identity-id {
    font-family: monospace;
    font-size: ${({ theme }) => theme.typography.small.size};
    background: ${({ theme }) => theme.colors.background.tertiary};
    padding: ${({ theme }) => theme.spacing.sm};
    border-radius: ${({ theme }) => theme.borderRadius.sm};
    margin: 0;
  }
`; 