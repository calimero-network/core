import React, { useState, useCallback, useEffect } from 'react';
import { NetworkId, setupWalletSelector, BrowserWallet } from '@near-wallet-selector/core';
import { setupMyNearWallet } from '@near-wallet-selector/my-near-wallet';
import { apiClient } from '@calimero-network/calimero-client';
import { styled } from 'styled-components';
import { useNavigate } from 'react-router-dom';
import Button from '../../common/Button';
import LoaderSpinner from '../../common/LoaderSpinner';
import Loading from '../../common/Loading';

const Wrapper = styled.div`
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 1rem;
  width: 100%;
`;

const ErrorMessage = styled.div`
  color: #dc2626;
  font-size: 1.1rem;
`;

const SuccessMessage = styled.div`
  color: #16a34a;
  font-size: 1.1rem;
`;

interface ProviderConfig {
  network?: string;
  rpcUrl?: string;
  walletConnectProjectId?: string;
}

interface Provider {
  name: string;
  type: string;
  description: string;
  configured: boolean;
  config?: ProviderConfig;
}

interface NearWalletProviderProps {
  provider: Provider;
}

export function NearWalletProvider({ provider }: NearWalletProviderProps) {
  const [error, setError] = useState<string | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const navigate = useNavigate();
  const [isLoading, setIsLoading] = useState(false);

  useEffect(() => {
    const urlParams = new URLSearchParams(window.location.search);
    const accountId = urlParams.get('account_id');
    const publicKey = urlParams.get('all_keys');

    console.log('accountId', accountId);
    console.log('publicKey', publicKey);

    if (accountId && publicKey) {
      addRootKey(publicKey, accountId);
    }else {
      connectWallet();
    }
  }, []);

  const connectWallet = useCallback(async () => {
    setError(null);
    setIsLoading(true);
    try {
      const network = provider.config?.network || 'testnet';
      const walletSelector = await setupWalletSelector({
        network: network as NetworkId,
        modules: [
          setupMyNearWallet()
        ],
      });

      const wallet = await walletSelector?.wallet('my-near-wallet') as BrowserWallet;
      if (!wallet) {
        throw new Error('No wallet selected');
      }

      const account = await wallet.getAccounts();
      if (account && account.length > 0) {
        wallet.signOut();
      }

      await wallet.signIn({
        contractId: '',
        failureUrl: window.location.origin + '/admin-dashboard/identity/root-key/',
      });

    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to connect wallet');
      console.error('Failed to connect wallet:', err);
    }
  }, [provider]);

  const addRootKey = useCallback(async (publicKey: string, accountId: string) => {
    try {
      const response = await apiClient.admin().addRootKey({
        public_key: publicKey,
        auth_method: 'near_wallet',
        provider_data: {
          account_id: accountId,
        },
      });

      if (response.error) {
        throw new Error(response.error.message);
      }

      console.log('response', response);

      setError(null);
      setMessage(response.data?.message);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to add root key');
      throw err;
    }
  }, []);

  return (
    <Wrapper>
      <h2>Connect {provider.name}</h2>
      {error && <ErrorMessage>{error}</ErrorMessage>}
      {message && <SuccessMessage>{message}</SuccessMessage>}
      {
        isLoading ? (
          <Loading />
        ) : (
          <Button onClick={() => navigate('/identity')} text="Back" />
        )
      }
    </Wrapper>
  );
} 
