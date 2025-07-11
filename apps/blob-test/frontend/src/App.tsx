import React from 'react';
import { ProtectedRoutesWrapper } from '@calimero-network/calimero-client';
import ChatPage from './pages/ChatPage';

export default function App() {
  return (
    <ProtectedRoutesWrapper>
      <ChatPage />
    </ProtectedRoutesWrapper>
  );
}