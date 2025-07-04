import React from 'react';
import { ProtectedRoutesWrapper } from '@calimero-network/calimero-client';
import BlobTestPage from './pages/BlobTestPage';

export default function App() {
  return (
    <ProtectedRoutesWrapper>
      <BlobTestPage />
    </ProtectedRoutesWrapper>
  );
} 