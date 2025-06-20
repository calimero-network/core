import React from 'react';
import styled, { css } from 'styled-components';

interface StyledButtonProps {
  $primary?: boolean;
  $rounded?: boolean;
  $size?: 'sm' | 'md' | 'lg' | 'full';
}

const StyledButton = styled.button<StyledButtonProps>`
  color: ${({ theme }) => theme.colors.text.primary};
  width: ${({ $size }) => {
    switch ($size) {
      case 'sm': return '100px';
      case 'md': return '150px';
      case 'lg': return '200px';
      case 'full': return '100%';
      default: return '100%';
    }
  }};
  display: flex;
  justify-content: center;
  align-items: center;
  gap: ${({ theme }) => theme.spacing.sm};
  height: 46px;
  font-size: ${({ theme }) => theme.typography.body.size};
  font-weight: ${({ theme }) => theme.typography.subtitle.weight};
  border-radius: ${({ theme, $rounded }) => $rounded ? theme.borderRadius.lg : theme.borderRadius.default};
  border: none;
  outline: none;
  padding: ${({ theme }) => theme.spacing.sm};
  cursor: pointer;
  transition: ${({ theme }) => theme.transitions.default};

  ${({ $primary, theme }) => $primary ? css`
    background-color: ${theme.colors.accent.primary};
    &:hover:not(:disabled) {
      background-color: ${theme.colors.accent.secondary};
    }
  ` : css`
    background-color: #6b7280;
    &:hover:not(:disabled) {
      opacity: 0.9;
    }
  `}

  &:disabled {
    cursor: not-allowed;
    opacity: 0.7;
    background-color: ${({ $primary, theme }) => 
      $primary ? theme.colors.accent.secondary : '#6b7280'};
  }
`;

interface ButtonProps extends Omit<React.ButtonHTMLAttributes<HTMLButtonElement>, 'size'> {
  children: React.ReactNode;
  primary?: boolean;
  rounded?: boolean;
  size?: 'sm' | 'md' | 'lg' | 'full';
}

const Button: React.FC<ButtonProps> = ({ 
  children, 
  primary = false,
  rounded = false,
  size = 'full',
  ...props 
}) => {
  return (
    <StyledButton 
      $primary={primary}
      $rounded={rounded}
      $size={size}
      {...props}
    >
      {children}
    </StyledButton>
  );
};

export default Button;