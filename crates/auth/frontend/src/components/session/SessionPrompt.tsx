import Button from '../common/Button';
import { clearAccessToken, clearRefreshToken, getAccessToken, getRefreshToken } from '@calimero-network/calimero-client';
import {
  SessionPromptContainer,
  Title,
  Description,
  ButtonGroup
} from './styles';

interface SessionPromptProps {
  onContinueSession: () => void;
  onStartNewSession: () => void;
}

export function SessionPrompt({ onContinueSession, onStartNewSession }: SessionPromptProps) {
  if (!getAccessToken() || !getRefreshToken()) {
    return null;
  }

  const handleStartNewSession = () => {
    clearAccessToken();
    clearRefreshToken();
    onStartNewSession();
  };

  return (
    <SessionPromptContainer>
      <Title>Welcome Back!</Title>
      <Description>
        You have an active session. Would you like to continue with your existing session or start a new one?
      </Description>
      <ButtonGroup>
        <Button onClick={handleStartNewSession}>
          New Session
        </Button>
        <Button onClick={onContinueSession} primary>
          Continue Session
        </Button>
      </ButtonGroup>
    </SessionPromptContainer>
  );
} 