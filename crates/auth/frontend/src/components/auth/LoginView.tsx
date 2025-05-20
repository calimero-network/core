import React, { useEffect, useState } from 'react';
import styled from '@emotion/styled';
import { useAuth } from '../../hooks/useAuth';
import { Provider, ChallengeRequest, ChallengeResponse, SignedMessage } from '../../types/auth';
import ProviderSelector from './ProviderSelector';
import * as api from '../../services/api';
import { NetworkId, setupWalletSelector } from '@near-wallet-selector/core';
import { setupMyNearWallet } from '@near-wallet-selector/my-near-wallet';

const Container = styled.div`
  max-width: 100%;
`;

const ErrorMessage = styled.div`
  background-color: #ffebee;
  color: #c62828;
  padding: 12px;
  border-radius: 4px;
  margin-bottom: 20px;
  text-align: center;
`;

const LoginView: React.FC = () => {
  const [providers, setProviders] = useState<Provider[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  
  const { login, isAuthenticated } = useAuth();
  
  // Load available providers
  useEffect(() => {
    const loadProviders = async () => {
      try {
        const availableProviders = await api.getProviders();
        setProviders(availableProviders);
        setLoading(false);
      } catch (err) {
        console.error('Failed to load providers:', err);
        setError('Failed to load authentication providers');
        setLoading(false);
      }
    };
    
    loadProviders();
  }, []);
  
  // Handle provider selection
  const handleProviderSelect = async (provider: Provider) => {
    setError(null);
    
    try {
      if (provider.name === 'near_wallet') {
        let challengeRequest = {
          provider: 'near_wallet',
          redirect_uri: window.location.href,
          client_id: 'web-browser',
        } as ChallengeRequest;

        try {
          let challengeResponse = await api.getChallenge(challengeRequest);
          // Handle the challenge response
          if (!challengeResponse || !challengeResponse.message) {
            console.error('Invalid challenge response:', challengeResponse);
            return;
          }
          console.log('Challenge response:', challengeResponse);

          const challengeNonce = challengeResponse.message.split(':')[1];
          const nonce = Buffer.from(challengeNonce, 'base64');

          const selector = await setupWalletSelector({
            network: challengeResponse.network as NetworkId,
            debug: true,
            modules: [
              setupMyNearWallet()
            ]
          });

          const wallet = await selector.wallet('my-near-wallet');

          const signature = await wallet.signMessage({
              message: challengeResponse.message,
              nonce,
              callbackUrl: window.location.href, 
              recipient: challengeResponse.recipient || 'calimero',
          }) as SignedMessage;

          console.log('Signature:', signature);
          
          // Create the token request payload
          const tokenPayload = {
            public_key: signature.publicKey,
            account_id: signature.accountId,
            message: challengeResponse.message,
            signature: signature.signature
          };
          
          console.log('Token request payload:', tokenPayload);
          
          // Request JWT token with the signature
          const tokenResponse = await api.requestToken('near_wallet', tokenPayload);
          console.log('Token response:', tokenResponse);

          // // Store the token and update auth state
          // if (tokenResponse.access_token) {
          //   localStorage.setItem('auth_token', tokenResponse.access_token);
          //   localStorage.setItem('refresh_token', tokenResponse.refresh_token);
          //   login(tokenResponse.access_token, tokenResponse.refresh_token);
          // }
          
        } catch (err) {
          console.error('Authentication error:', err);
          setError(err instanceof Error ? err.message : 'Authentication failed');
        }
      } else {
        // For other providers, add implementation here
        setError(`Provider ${provider.name} is not implemented yet`);
      }
    } catch (err) {
      console.error('Authentication error:', err);
      setError(err instanceof Error ? err.message : 'Authentication failed');
    }
  };
  
  // Show authentication success
  if (isAuthenticated) {
    return (
      <Container>
        <div style={{ textAlign: 'center', padding: '20px' }}>
          <h2>Authentication Successful</h2>
          <p>You have successfully authenticated.</p>
        </div>
      </Container>
    );
  }
  
  if (loading) {
    return <div>Loading...</div>;
  }

  if (error) {
    return <ErrorMessage>{error}</ErrorMessage>;
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