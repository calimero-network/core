import React from 'react';
import { ThemeProvider as StyledThemeProvider } from 'styled-components';
import theme from './theme';
import { GlobalStyles } from './globalStyles';

interface ThemeProviderProps {
  children: React.ReactNode;
}

export function ThemeProvider({ children }: ThemeProviderProps) {
  return (
    <StyledThemeProvider theme={theme}>
      <GlobalStyles theme={theme} />
      {children}
    </StyledThemeProvider>
  );
}

export default ThemeProvider; 