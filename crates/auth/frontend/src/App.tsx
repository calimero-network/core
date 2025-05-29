import React from 'react';
import { AuthProvider } from './contexts/AuthContext';
import LoginView from './components/auth/LoginView';
import Layout from './components/common/Layout';

function App() {
  return (
    <AuthProvider>
      <Layout>
        <LoginView />
      </Layout>
    </AuthProvider>
  );
}

export default App;