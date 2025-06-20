import { useState } from 'react';
import { getStoredUrlParam } from '../utils/urlParams';
import { apiClient } from '@calimero-network/calimero-client';

export const PROTOCOLS = ['near', 'starknet', 'icp', 'stellar', 'ethereum'] as const;
export const PROTOCOL_DISPLAY = {
  near: 'NEAR',
  starknet: 'Starknet',
  icp: 'ICP',
  stellar: 'Stellar',
  ethereum: 'Ethereum'
} as const;

export type Protocol = typeof PROTOCOLS[number];

interface UseContextCreationReturn {
  isLoading: boolean;
  error: string | null;
  showInstallPrompt: boolean;
  selectedProtocol: Protocol | null;
  setSelectedProtocol: (protocol: Protocol | null) => void;
  checkAndInstallApplication: (applicationId: string, applicationPath: string) => Promise<boolean>;
  handleContextCreation: () => Promise<{ contextId: string; memberPublicKey: string } | undefined>;
  handleInstallCancel: () => void;
}

export function useContextCreation(): UseContextCreationReturn {
  const [isLoading, setIsLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [showInstallPrompt, setShowInstallPrompt] = useState(false);
  const [applicationMismatch, setApplicationMismatch] = useState(false);
  const [selectedProtocol, setSelectedProtocol] = useState<Protocol | null>(null);

  const checkAndInstallApplication = async (applicationId: string, applicationPath: string) => {
    try {
      if (!applicationId || !applicationPath || !selectedProtocol) {
        throw new Error('Missing required parameters');
      }

      const application = await apiClient.node().getInstalledApplicationDetails(applicationId);
      console.log('application', application);
      
      if (application.data) {
        // Application doesn't exist, try to install with expected ID
        const installResponse = await apiClient.node().installApplication(applicationPath, new Uint8Array(), applicationId);
        console.log('installResponse', installResponse);

        if (installResponse.error) {
          console.log('installResponse.error', installResponse.error);
          if(installResponse.error.message === 'fatal: blob hash mismatch') {
            console.log('application mismatch');
            setApplicationMismatch(true);
            setShowInstallPrompt(true);
            return false;
          }

          throw new Error(installResponse.error.message);
        }
        return true;
      }
      // Application exists
      return true;
    } catch (err) {
      throw err;
    }
  };

  const handleContextCreation = async () => {
    setIsLoading(true);
    setError(null);
    
    try {
      const applicationPath = getStoredUrlParam('application-path');
      const applicationId = getStoredUrlParam('application-id');
      
      if (!applicationPath || !applicationId || !selectedProtocol) {
        throw new Error('Missing required parameters');
      }

      // Install application without expected ID
      const installResponse = await apiClient.node().installApplication(applicationPath, new Uint8Array());
      if (installResponse.error) {
        setError(installResponse.error.message);
        return;
      }
      const newApplicationId = installResponse.data.applicationId;
      localStorage.setItem('application-id', JSON.stringify(newApplicationId));
      // Create context
      const createContextResponse = await apiClient.node().createContext(newApplicationId, selectedProtocol);
      console.log('createContextResponse', createContextResponse);
      if (createContextResponse.error) {
        setError(createContextResponse.error.message);
        return;
      }

      // Handle successful context creation
      if (createContextResponse.data) {
        const { contextId, memberPublicKey } = createContextResponse.data;
        setSelectedProtocol(null);
        setShowInstallPrompt(false);
        setApplicationMismatch(false);
        return { contextId, memberPublicKey };
      }
    } catch (err: any) {
      setError(err.message || 'Failed to install application');
    } finally {
      setIsLoading(false);
    }
  };

  const handleInstallCancel = () => {
    setShowInstallPrompt(false);
    setApplicationMismatch(false);
  };

  return {
    isLoading,
    error,
    showInstallPrompt,
    selectedProtocol,
    setSelectedProtocol,
    checkAndInstallApplication,
    handleContextCreation,
    handleInstallCancel
  };
} 