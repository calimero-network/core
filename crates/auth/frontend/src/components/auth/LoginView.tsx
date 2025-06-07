import React, { useState, useCallback, useEffect } from 'react';
import { useAuth } from '../../hooks/useAuth';
import { Provider } from '../../types/auth';
import { Context, ContextIdentity } from '../../types/api';
import * as api from '../../services/api';
import { AuthStorage } from '../../services/authStorage';
import { Container, Button, ButtonGroup, ErrorMessage, SessionPrompt } from '../auth/styles';
import ProviderSelector from './ProviderSelector';
import { ContextSelector } from '../ContextSelector';
import { setupWalletSelector } from '@near-wallet-selector/core';
import { setupMyNearWallet } from '@near-wallet-selector/my-near-wallet';
import { Buffer } from 'buffer';

interface SignedMessage {
  accountId: string;
  publicKey: string;
  signature: string;
}

const LoginView: React.FC = () => {
  const [providers, setProviders] = useState<Provider[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [showProviders, setShowProviders] = useState(false);
  const [showContextSelector, setShowContextSelector] = useState(false);
  const [rootToken, setRootToken] = useState<string | null>(null);
  
  const { login, isAuthenticated } = useAuth();

  // Load providers
  const loadProviders = useCallback(async () => {
    try {
      const availableProviders = await api.getProviders();
      setProviders(availableProviders);
    } catch (err) {
      console.error('Failed to load providers:', err);
      setError('Failed to load authentication providers');
    }
  }, []);
  
  useEffect(() => {
    const checkExistingSession = async () => {
      const existingRootToken = AuthStorage.getRootToken();
      const clientTokens = AuthStorage.getClientTokens();
      
      if (existingRootToken && clientTokens?.refresh_token) {
        // If there's an existing token, show providers only if user explicitly chooses new login
        setShowProviders(false);
        setRootToken(existingRootToken);
      } else {
        // If no existing token, show providers directly
        setShowProviders(true);
        await loadProviders();
      }
      setLoading(false);
    };
    
    checkExistingSession();
  }, [loadProviders]);
  
  const handleContinueSession = () => {
    const existingRootToken = AuthStorage.getRootToken();
    if (existingRootToken) {
      setRootToken(existingRootToken);
      setShowContextSelector(true);
    }
  };

  const handleNewLogin = async () => {
    // Clear existing tokens before starting new login
    AuthStorage.clearTokens();
    setRootToken(null);
    setShowProviders(true);
    await loadProviders();
  };

  const handleProviderSelect = async (provider: Provider) => {
    try {
      if (provider.name === 'near_wallet') {
        // Get challenge for NEAR wallet
        const challengeResponse = await api.getChallenge();
        console.log('Challenge response:', challengeResponse);
        
        // Generate nonce
        const nonceArray = new Uint8Array(32);
        window.crypto.getRandomValues(nonceArray);
        const nonceBuffer = Buffer.from(nonceArray);
        const nonceString = nonceBuffer.toString('base64');

        // Setup NEAR wallet
        const selector = await setupWalletSelector({
          network: 'testnet',
          modules: [setupMyNearWallet()]
        });

        const wallet = await selector.wallet('my-near-wallet');
        
        // Sign the challenge
        const signature = await wallet.signMessage({
          message: challengeResponse.challenge,
          nonce: nonceBuffer,
          recipient: 'calimero',
          callbackUrl: window.location.href
        }) as SignedMessage;

        console.log('Signature:', signature);

        // Create token request
        const tokenPayload = {
          auth_method: 'near_wallet',
          public_key: signature.publicKey,
          wallet_address: signature.accountId,
          client_name: 'NEAR Wallet',
          message: challengeResponse.challenge,
          signature: signature.signature,
          timestamp: Date.now(),
          nonce: nonceString,
          recipient: 'calimero',
          callback_url: window.location.href
        };

        console.log('Token request payload:', tokenPayload);

        // Get root token
        const tokenResponse = await api.requestToken(tokenPayload);
        console.log('Token response:', tokenResponse);

        if (tokenResponse.access_token) {
          // Store root token and show context selector
          AuthStorage.setRootToken(tokenResponse.access_token);
          setRootToken(tokenResponse.access_token);
          setShowContextSelector(true);
        } else {
          throw new Error('Failed to get access token');
        }
      } else {
        setError(`Provider ${provider.name} is not implemented yet`);
      }
    } catch (err) {
      console.error('Authentication error:', err);
      setError(err instanceof Error ? err.message : 'Authentication failed');
    }
  };

  const handleContextAndIdentitySelect = async (context: Context, identity: ContextIdentity) => {
    try {
      // Generate client key using context and identity
      const response = await api.generateClientKey(rootToken!, {
        context_id: context.id,
        context_identity: identity
      });

      console.log('generateClientKey response', response);

      if (response.access_token && response.refresh_token) {
        // Store the client tokens
        AuthStorage.setClientTokens(response);
        // Complete login with the client tokens
        await login(response.access_token, response.refresh_token);
      } else {
        throw new Error('Failed to generate client key');
      }
    } catch (err) {
      console.error('Failed to generate client key:', err);
      setError(err instanceof Error ? err.message : 'Failed to generate client key');
    }
  };
  
  if (loading) {
    return <div>Loading...</div>;
  }

  if (error) {
    return <ErrorMessage>{error}</ErrorMessage>;
  }

  if (showContextSelector && rootToken) {
    return (
      <Container>
        <ContextSelector onComplete={handleContextAndIdentitySelect} />
      </Container>
    );
  }

  if (!showProviders && AuthStorage.getRootToken()) {
    return (
      <Container>
        <SessionPrompt>
          <h2>Welcome Back!</h2>
          <p>We noticed you have an existing session. Would you like to continue with it?</p>
          <ButtonGroup>
            <Button className="primary" onClick={handleContinueSession}>
              Continue Session
            </Button>
            <Button className="secondary" onClick={handleNewLogin}>
              New Login
            </Button>
          </ButtonGroup>
        </SessionPrompt>
      </Container>
    );
  }

  return (
    <Container>
      <ProviderSelector 
        providers={providers} 
        onProviderSelect={handleProviderSelect}
        loading={loading}
      />
    </Container>
  );
};

export default LoginView;