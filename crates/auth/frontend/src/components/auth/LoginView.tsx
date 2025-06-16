import React, { useState, useCallback, useEffect } from 'react';
import { Container, ButtonGroup, ErrorMessage, SessionPrompt } from '../auth/styles';
import Button from '../common/Button';
import ProviderSelector from './ProviderSelector';
import { ContextSelector } from '../context/ContextSelector';
import { NetworkId, setupWalletSelector } from '@near-wallet-selector/core';
import { setupMyNearWallet } from '@near-wallet-selector/my-near-wallet';
import { Buffer } from 'buffer';
import { handleUrlParams, getStoredUrlParam, clearStoredUrlParams } from '../../utils/urlParams';
import { apiClient, clearAccessToken, clearRefreshToken, getAccessToken, getRefreshToken, setAccessToken, setRefreshToken } from '@calimero-network/calimero-client';
import { Provider } from '@calimero-network/calimero-client/lib/api/authApi';

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

  // Load providers
  const loadProviders = useCallback(async () => {
    try {
      
      const availableProviders = await apiClient.auth().getProviders();

      if (availableProviders.error) {
        setError(availableProviders.error.message);
        return;
      }

      console.log('availableProviders', availableProviders);
      setProviders(availableProviders.data.providers);
    } catch (err) {
      console.error('Failed to load providers:', err);
      setError('Failed to load authentication providers');
    }
  }, []);

  const checkIfTokenIsValid = async (accessToken: string, refreshToken: string) => {
    try {
      const response = await apiClient.auth().refreshToken({
        access_token: accessToken,
        refresh_token: refreshToken
      });
      console.log('refreshToken response', response);
      
      if (response.error?.message?.includes('Access token still valid')) {
        // Current token is still valid, let user choose to continue or start new
        setShowProviders(false);
        return true;
      }

      if (response.data?.access_token && response.data?.refresh_token) {
        // Got new tokens, store them and let user choose to continue or start new
        setAccessToken(response.data.access_token);
        setRefreshToken(response.data.refresh_token)
        setShowProviders(false);
        return true;
      }

      // Any other case is an error
      throw new Error(response.error?.message || 'Failed to validate token');
    } catch (err) {
      console.error('Token validation failed:', err);
      clearAccessToken();
      clearRefreshToken();
      setShowProviders(true);
      await loadProviders();
      return false;
    }
  };

  const checkExistingSession = async () => {
    const existingAccessToken = getAccessToken();
    const existingRefreshToken = getRefreshToken();
    console.log('existingAccessToken', existingAccessToken);
    console.log('existingRefreshToken', existingRefreshToken);
    
    if (existingAccessToken && existingRefreshToken) {
      checkIfTokenIsValid(existingAccessToken, existingRefreshToken);
    } else {
      setShowProviders(true);
      await loadProviders();
    }
    setLoading(false);
  };
  
  useEffect(() => {    
    checkExistingSession();
  }, [loadProviders]);
  
  useEffect(() => {
    // // Handle URL parameters on mount
    // const urlParams = handleUrlParams();
    // console.log('Stored URL parameters:', urlParams);
    
    // // Check for mandatory callback URL
    // const callback = getStoredUrlParam('callback-url');
    // if (!callback) {
    //   setError('Missing required callback URL parameter');
    //   setLoading(false);
    // }
  }, []);
  
  const handleContinueSession = () => {
    const existingAccessToken = getAccessToken();
    const existingRefreshToken = getRefreshToken();
    if (existingAccessToken && existingRefreshToken) {
      setRootToken(existingAccessToken);
      setShowContextSelector(true);
    }
  };

  const handleNewLogin = async () => {
    // Clear existing tokens before starting new login
    clearAccessToken();
    clearRefreshToken();
    setRootToken(null);
    setShowProviders(true);
    await loadProviders();
  };

  const handleProviderSelect = async (provider: Provider) => {
    try {
      if (provider.name === 'near_wallet') {
        // Get challenge for NEAR wallet
        const challengeResponse = await apiClient.auth().getChallenge();
        console.log('Challenge response:', challengeResponse);

        if (challengeResponse.error) {
          setError(challengeResponse.error.message);
          return;
        }
        
        // Setup NEAR wallet
        const selector = await setupWalletSelector({
          network: provider.config?.network as NetworkId,
          modules: [setupMyNearWallet()]
        });

        const wallet = await selector.wallet('my-near-wallet');
        
        // Sign the challenge
        const signature = await wallet.signMessage({
          message: challengeResponse.data.challenge,
          nonce: Buffer.from(challengeResponse.data.nonce, 'base64'),
          recipient: 'calimero',
          callbackUrl: window.location.href
        }) as SignedMessage;

        // Create token request with separated provider-specific data
        const tokenPayload = {
          // Common fields
          auth_method: 'near_wallet',
          public_key: signature.publicKey,
          client_name: 'NEAR Wallet',
          timestamp: Date.now(),
          permissions: [],
          
          // Provider-specific data
          provider_data: {
            wallet_address: signature.accountId,
            message: challengeResponse.data.challenge,
            signature: signature.signature,
            recipient: 'calimero'
          }
        };
        // Get root token
        const tokenResponse = await apiClient.auth().requestToken(tokenPayload);

        if (tokenResponse.error) {
          setError(tokenResponse.error.message);
          return;
        }

        if (tokenResponse.data.access_token && tokenResponse.data.refresh_token) {
          // Store root token and show context selector
          setAccessToken(tokenResponse.data.access_token);
          setRefreshToken(tokenResponse.data.refresh_token);
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

  const handleContextAndIdentitySelect = async (context: string, identity: string) => {
    try {
      let permissions: string[] = [];
      const permissionsParam = getStoredUrlParam('permissions');
      if (permissionsParam) {
        permissions = permissionsParam.split(',');
      }

      // Generate client key using context and identity
      const response = await apiClient.auth().generateClientKey(rootToken!, {
        context_id: context,
        context_identity: identity,
        permissions
      });

      if (response.error) {
        setError(response.error.message);
        return;
      }

      if (response.data.access_token && response.data.refresh_token) {

        // Get the callback URL from localStorage
        const callback = getStoredUrlParam('callback-url');
        if (callback) {
          // Create return URL with tokens as query parameters
          const returnUrl = new URL(callback);
          returnUrl.searchParams.set('access_token', response.data.access_token);
          returnUrl.searchParams.set('refresh_token', response.data.refresh_token);
          
          // Clear stored URL parameters before redirecting
          clearStoredUrlParams();
          
          // Redirect to callback URL
          window.location.href = returnUrl.toString();
        }
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
    return (
      <Container>
        <ErrorMessage>{error}</ErrorMessage>
        {error === 'Missing required callback URL parameter' && (
          <p style={{ marginTop: '1rem', textAlign: 'center' }}>
            Please provide a callback URL in the query parameters (e.g., ?callback=your_url or ?redirect_uri=your_url)
          </p>
        )}
      </Container>
    );
  }

  const handleBack = () => {
    setShowContextSelector(false);
    checkExistingSession();
  };

  return (
    <Container>
      {error && <ErrorMessage>{error}</ErrorMessage>}
      
      {!loading && !showProviders && !showContextSelector && getAccessToken() && getRefreshToken() && (
        <SessionPrompt>
          <h2>Welcome Back!</h2>
          <p>We noticed you have an existing session. Would you like to continue with it?</p>
          <ButtonGroup>
            <Button onClick={handleContinueSession} primary>
              Continue Session
            </Button>
            <Button onClick={handleNewLogin}>
              New Login
            </Button>
          </ButtonGroup>
        </SessionPrompt>
      )}

      {showProviders && (
        <ProviderSelector
          providers={providers}
          onProviderSelect={handleProviderSelect}
          loading={loading}
        />
      )}

      {showContextSelector && (
        <ContextSelector
          onComplete={handleContextAndIdentitySelect}
          onBack={handleBack}
        />
      )}
    </Container>
  );
};

export default LoginView;