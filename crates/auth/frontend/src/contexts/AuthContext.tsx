import React, { createContext, useState, useEffect, ReactNode } from 'react';
import { AuthState, AuthContextType } from '../types/auth';

// Default state
const initialState: AuthState = {
  isAuthenticated: false,
  loading: false,
  error: null
};

// Create context with default values
export const AuthContext = createContext<AuthContextType>({
  ...initialState,
  login: async () => false,
  logout: () => {}
});

export const AuthProvider: React.FC<{ children: ReactNode }> = ({ children }) => {
  const [state, setState] = useState<AuthState>(initialState);

  // Login function
  const login = async (accessToken: string, refreshToken: string): Promise<boolean> => {
    try {
      // Store tokens
      localStorage.setItem('auth_token', accessToken);
      localStorage.setItem('refresh_token', refreshToken);
      
      setState({
        isAuthenticated: true,
        loading: false,
        error: null
      });
      return true;
    } catch (error) {
      console.error('Login failed:', error);
      localStorage.removeItem('auth_token');
      localStorage.removeItem('refresh_token');
      setState({
        isAuthenticated: false,
        loading: false,
        error: error instanceof Error ? error.message : 'Login failed'
      });
      return false;
    }
  };

  // Logout function
  const logout = () => {
    localStorage.removeItem('auth_token');
    localStorage.removeItem('refresh_token');
    
    setState({
      isAuthenticated: false,
      loading: false,
      error: null
    });
  };

  return (
    <AuthContext.Provider value={{
      ...state,
      login,
      logout
    }}>
      {children}
    </AuthContext.Provider>
  );
};