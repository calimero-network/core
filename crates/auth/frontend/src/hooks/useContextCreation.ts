import { useState } from 'react';
import { getStoredUrlParam } from '../utils/urlParams';
import { apiClient } from '@calimero-network/calimero-client';

export type Protocol = 'NEAR' | 'Starknet' | 'ICP' | 'Stellar' | 'Ethereum';

interface UseContextCreationReturn {
  isLoading: boolean;
  error: string | null;
  showInstallPrompt: boolean;
  selectedProtocol: Protocol | null;
  setSelectedProtocol: (protocol: Protocol) => void;
  createContext: () => Promise<void>;
  handleInstallConfirm: () => Promise<void>;
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
      const application = await apiClient.node().getInstalledApplicationDetails(applicationId);
      console.log('application', application);
      
      if (!application) {
        // Application doesn't exist, try to install with expected ID
        try {
          await apiClient.node().installApplication(applicationPath, new Uint8Array(), applicationId);
          // Application installed successfully
          return true;
        } catch (err: any) {
          if (err.message?.includes('400')) {
            // Application ID mismatch
            setApplicationMismatch(true);
            setShowInstallPrompt(true);
            return false;
          }
          throw err;
        }
      }
      // Application exists
      return true;
    } catch (err) {
      throw err;
    }
  };

  const createContext = async () => {
    setIsLoading(true);
    setError(null);
    
    try {
      if (!selectedProtocol) {
        throw new Error('Please select a protocol before creating a context');
      }

      const applicationId = getStoredUrlParam('applicationId');
      const applicationPath = getStoredUrlParam('applicationPath');
      
      if (!applicationId || !applicationPath) {
        throw new Error('Missing required parameters: applicationId or applicationPath');
      }

      // Check and install application if needed
      const hasApplication = await checkAndInstallApplication(applicationId, applicationPath);
      
      if (!hasApplication && !applicationMismatch) {
        // Application doesn't exist and no mismatch - general error
        throw new Error('Failed to install application');
      }

      if (!applicationMismatch) {
        // Create context only if we have the application and there's no mismatch
        await apiClient.node().createContext(applicationId, selectedProtocol);
      }
    } catch (err: any) {
      setError(err.message || 'Failed to create context');
    } finally {
      setIsLoading(false);
    }
  };

  const handleInstallConfirm = async () => {
    setIsLoading(true);
    setError(null);
    
    try {
      const applicationPath = getStoredUrlParam('applicationPath');
      const applicationId = getStoredUrlParam('applicationId');
      
      if (!applicationPath || !applicationId) {
        throw new Error('Missing required parameters');
      }

      // Install application without expected ID
      const installResponse = await apiClient.node().installApplication(applicationPath, new Uint8Array());
      if (installResponse.error) {
        setError(installResponse.error.message);
        return;
      }
      const newApplicationId = installResponse.data.applicationId;

      if (!selectedProtocol) {
        throw new Error('Please select a protocol before creating a context');
      }

      // Create context
      const createContextResponse = await apiClient.node().createContext(newApplicationId, selectedProtocol);
      if (createContextResponse.error) {
        setError(createContextResponse.error.message);
        return;
      }

      setShowInstallPrompt(false);
      setApplicationMismatch(false);
    } catch (err: any) {
      setError(err.message || 'Failed to install application');
    } finally {
      setIsLoading(false);
    }
  };

  const handleInstallCancel = () => {
    setShowInstallPrompt(false);
    setApplicationMismatch(false);
    setError('Context creation cancelled');
  };

  return {
    isLoading,
    error,
    showInstallPrompt,
    selectedProtocol,
    setSelectedProtocol,
    createContext,
    handleInstallConfirm,
    handleInstallCancel
  };
} 