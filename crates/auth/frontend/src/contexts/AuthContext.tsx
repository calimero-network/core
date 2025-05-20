import React, { createContext, useState, useEffect, ReactNode } from 'react';
import { AuthState, AuthContextType } from '../types/auth';
import * as api from '../services/api';

// Default state
const initialState: AuthState = {
  isAuthenticated: false,
  loading: true,
  userId: null,
  permissions: [],
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

  // Check if we're authenticated on mount
  useEffect(() => {
    const checkAuth = async () => {
      const token = localStorage.getItem('auth_token');
      if (!token) {
        setState(prev => ({ ...prev, loading: false }));
        return;
      }

      try {
        const { valid, userId, permissions } = await api.verifyToken();
        
        if (valid && userId) {
          setState({
            isAuthenticated: true,
            loading: false,
            userId,
            permissions,
            error: null
          });
        } else {
          // Clear invalid token
          localStorage.removeItem('auth_token');
          localStorage.removeItem('refresh_token');
          setState({
            isAuthenticated: false,
            loading: false,
            userId: null,
            permissions: [],
            error: null
          });
        }
      } catch (error) {
        console.error('Auth verification failed:', error);
        localStorage.removeItem('auth_token');
        localStorage.removeItem('refresh_token');
        setState({
          isAuthenticated: false,
          loading: false,
          userId: null,
          permissions: [],
          error: 'Authentication verification failed'
        });
      }
    };

    checkAuth();
  }, []);

  // Login function
  const login = async (accessToken: string, refreshToken: string): Promise<boolean> => {
    setState(prev => ({ ...prev, loading: true, error: null }));

    try {
      // Store tokens
      localStorage.setItem('auth_token', accessToken);
      localStorage.setItem('refresh_token', refreshToken);
      
      // Verify to get user info
      const { valid, userId, permissions } = await api.verifyToken();
      
      if (valid && userId) {
        setState({
          isAuthenticated: true,
          loading: false,
          userId,
          permissions,
          error: null
        });
        return true;
      }
      
      throw new Error('Failed to authenticate');
    } catch (error) {
      console.error('Login failed:', error);
      setState({
        isAuthenticated: false,
        loading: false,
        userId: null,
        permissions: [],
        error: error instanceof Error ? error.message : 'Authentication failed'
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
      userId: null,
      permissions: [],
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