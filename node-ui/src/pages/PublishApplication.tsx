import React, { useState, useEffect, useRef, ChangeEvent } from 'react';
import { Navigation } from '../components/Navigation';
import { FlexLayout } from '../components/layout/FlexLayout';
import {
  Account,
  BrowserWallet,
  NetworkId,
  setupWalletSelector,
} from '@near-wallet-selector/core';
import { setupMyNearWallet } from '@near-wallet-selector/my-near-wallet';
import { useRPC } from '../hooks/useNear';
import axios from 'axios';

import * as nearAPI from 'near-api-js';
import { Package } from './Applications';
import PageContentWrapper from '../components/common/PageContentWrapper';
import PublishApplicationTable from '../components/publishApplication/PublishApplicationTable';
import { useNavigate } from 'react-router-dom';

const BLOBBY_IPFS = 'https://blobby-public.euw3.prod.gcp.calimero.network';
const NEAR_NETWORK = 'testnet';
const NEAR_CONTRACT_ID = 'calimero-package-manager.testnet';

export interface PackageInfo {
  name: string;
  description: string;
  repository: string;
}

export interface ReleaseInfo {
  name: string;
  version: string;
  notes: string;
  path: string;
  hash: string;
}

export interface DeployStatus {
  title: string;
  message: string;
  error: boolean;
}

export default function PublishApplicationPage() {
  const navigate = useNavigate();
  const fileInputRef = useRef<HTMLInputElement | null>(null);
  const { getPackages } = useRPC();
  const [ipfsPath, setIpfsPath] = useState('');
  const [fileHash, setFileHash] = useState('');
  const [packages, setPackages] = useState<Package[]>([]);
  const [deployerAccount, setDeployerAccount] = useState<Account>();
  const [showStatusModal, setShowStatusModal] = useState(false);
  const [isLoading, setIsLoading] = useState(false);
  const [packageInfo, setPackageInfo] = useState<PackageInfo>({
    name: '',
    description: '',
    repository: '',
  });
  const [deployStatus, setDeployStatus] = useState<DeployStatus>({
    title: '',
    message: '',
    error: false,
  });
  const [releaseInfo, setReleaseInfo] = useState<ReleaseInfo>({
    name: '',
    version: '',
    notes: '',
    path: '',
    hash: '',
  });

  useEffect(() => {
    (async () => {
      setPackages(await getPackages());
    })();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [ipfsPath]);

  useEffect(() => {
    setReleaseInfo((prevState) => ({
      ...prevState,
      path: ipfsPath,
      hash: fileHash,
    }));
  }, [ipfsPath, fileHash]);

  useEffect(() => {
    const fetchWalletAccounts = async () => {
      const selector = await setupWalletSelector({
        network: NEAR_NETWORK,
        modules: [setupMyNearWallet()],
      });
      const wallet = await selector.wallet('my-near-wallet');
      const accounts = await wallet.getAccounts();
      if (accounts.length !== 0) {
        setDeployerAccount(accounts[0]);
      }
    };
    fetchWalletAccounts();
  }, []);

  const addWalletAccount = async () => {
    const selector = await setupWalletSelector({
      network: NEAR_NETWORK as NetworkId,
      modules: [setupMyNearWallet()],
    });
    const wallet: BrowserWallet = await selector.wallet('my-near-wallet');
    await wallet.signOut();
    wallet.signIn({ contractId: NEAR_CONTRACT_ID });
  };

  const handleFileChange = (event: ChangeEvent<HTMLInputElement>) => {
    const file = event.target.files && event.target.files[0];
    if (file && file.name.endsWith('.wasm')) {
      const reader = new FileReader();
      reader.onload = async (e) => {
        if (!e?.target?.result) {
          console.error('Failed to read file or file content is not available');
          return;
        }

        const arrayBuffer = new Uint8Array(e.target.result as ArrayBufferLike);
        const bytes = new Uint8Array(arrayBuffer);
        const blob = new Blob([bytes], { type: 'application/wasm' });

        const hashBuffer = await crypto.subtle.digest(
          'SHA-256',
          await blob.arrayBuffer(),
        );

        const hashHex = Array.from(new Uint8Array(hashBuffer))
          .map((byte) => byte.toString(16).padStart(2, '0'))
          .join('');

        setFileHash(hashHex);

        await axios
          .post(BLOBBY_IPFS, blob)
          .then((response) => {
            setIpfsPath(`${BLOBBY_IPFS}/${response.data.cid}`);
          })
          .catch((error) => {
            console.error('Error occurred while uploading the file:', error);
          });
      };

      reader.onerror = (e: ProgressEvent<FileReader>) => {
        console.error(
          'Error occurred while reading the file:',
          e.target?.error,
        );
        return;
      };

      reader.readAsArrayBuffer(file);
    }
  };

  const addPackage = async () => {
    const selector = await setupWalletSelector({
      network: NEAR_NETWORK,
      modules: [setupMyNearWallet()],
    });
    const wallet = await selector.wallet('my-near-wallet');
    try {
      const res = await wallet.signAndSendTransaction({
        signerId: deployerAccount ? deployerAccount.accountId : '',
        actions: [
          {
            type: 'FunctionCall',
            params: {
              methodName: 'add_package',
              args: {
                name: packageInfo.name,
                description: packageInfo.description,
                repository: packageInfo.repository,
              },
              gas: nearAPI.utils.format.parseNearAmount('0.00000000003') ?? '0',
              deposit: '',
            },
          },
        ],
      });

      // Check if transaction was successful
      const status = res?.status;
      const isSuccess = status && status !== 'Failure';

      if (isSuccess) {
        setDeployStatus({
          title: 'Package added successfully',
          message: `Package ${packageInfo.name} added successfully`,
          error: false,
        });
      }
    } catch (error) {
      let errorMessage = '';

      if (error instanceof Error) {
        errorMessage =
          JSON.parse(error.message).kind?.kind?.FunctionCallError
            ?.ExecutionError ?? 'An error occurred while publishing package';
      }

      setDeployStatus({
        title: 'Error',
        message: errorMessage,
        error: true,
      });
    }
    setShowStatusModal(true);
  };

  const addRelease = async () => {
    const selector = await setupWalletSelector({
      network: NEAR_NETWORK,
      modules: [setupMyNearWallet()],
    });
    const wallet = await selector.wallet('my-near-wallet');
    try {
      const res = await wallet.signAndSendTransaction({
        signerId: deployerAccount ? deployerAccount.accountId : '',
        actions: [
          {
            type: 'FunctionCall',
            params: {
              methodName: 'add_release',
              args: {
                package_id: packageInfo.name,
                version: releaseInfo.version,
                notes: releaseInfo.notes,
                path: releaseInfo.path,
                hash: releaseInfo.hash,
              },
              gas: nearAPI.utils.format.parseNearAmount('0.00000000003') ?? '0',
              deposit: '',
            },
          },
        ],
      });

      // Check if transaction was successful
      const status = res?.status;
      const isSuccess = status && status !== 'Failure';

      if (isSuccess) {
        setDeployStatus({
          title: 'Release added successfully',
          message: `Release ${releaseInfo.version} added successfully`,
          error: false,
        });
      }
    } catch (error) {
      let errorMessage = '';

      if (error instanceof Error) {
        errorMessage =
          JSON.parse(error.message).kind?.kind?.FunctionCallError
            ?.ExecutionError ?? 'An error occurred while publishing release';
      }

      setDeployStatus({
        title: 'Error',
        message: errorMessage,
        error: true,
      });
    }
    setShowStatusModal(true);
  };

  const closeStatusModal = () => {
    setShowStatusModal(false);
    if (!deployStatus.error) {
      navigate('/applications');
    }
  };

  const publishApplication = async () => {
    setIsLoading(true);
    if (packages.find((p) => p.name === packageInfo.name)) {
      await addRelease();
    } else {
      await addPackage();
    }
    setIsLoading(false);
  };

  return (
    <FlexLayout>
      <Navigation />
      <PageContentWrapper>
        <PublishApplicationTable
          packageInfo={packageInfo}
          setPackageInfo={setPackageInfo}
          releaseInfo={releaseInfo}
          setReleaseInfo={setReleaseInfo}
          fileInputRef={fileInputRef}
          handleFileChange={handleFileChange}
          deployerAccount={deployerAccount}
          addWalletAccount={addWalletAccount}
          publishApplication={publishApplication}
          showStatusModal={showStatusModal}
          closeStatusModal={closeStatusModal}
          deployStatus={deployStatus}
          isLoading={isLoading}
        />
      </PageContentWrapper>
    </FlexLayout>
  );
}
