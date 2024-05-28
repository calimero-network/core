import React, { useState, useEffect, useRef, ChangeEvent } from "react";
import { Navigation } from "../components/Navigation";
import { FlexLayout } from "../components/layout/FlexLayout";
import {
  Account,
  BrowserWallet,
  setupWalletSelector,
} from "@near-wallet-selector/core";
import { setupMyNearWallet } from "@near-wallet-selector/my-near-wallet";
import { useRPC } from "../hooks/useNear";
import axios from "axios";

import * as nearAPI from "near-api-js";
import { Package } from "./Applications";
import PageContentWrapper from "../components/common/PageContentWrapper";
import PublishApplicationTable from "../components/publishApplication/PublishApplicationTable";
import { useNavigate } from "react-router-dom";
import { isFinalExecutionStatus } from "../utils/wallet";

const BLOBBY_IPFS = "https://blobby-public.euw3.prod.gcp.calimero.network";

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

export default function PublishApplication() {
  const navigate = useNavigate();
  const fileInputRef = useRef<HTMLInputElement | null>(null);
  const { getPackages } = useRPC();
  const [ipfsPath, setIpfsPath] = useState("");
  const [fileHash, setFileHash] = useState("");
  const [packages, setPackages] = useState<Package[]>([]);
  const [deployerAccount, setDeployerAccount] = useState<Account>();
  const [showStatusModal, setShowStatusModal] = useState(false);
  const [isLoading, setIsLoading] = useState(false);
  const [packageInfo, setPackageInfo] = useState<PackageInfo>({
    name: "",
    description: "",
    repository: "",
  });
  const [deployStatus, setDeployStatus] = useState<DeployStatus>({
    title: "",
    message: "",
    error: false,
  });
  const [releaseInfo, setReleaseInfo] = useState<ReleaseInfo>({
    name: "",
    version: "",
    notes: "",
    path: "",
    hash: "",
  });

  useEffect(() => {
    (async () => {
      setPackages(await getPackages());
    })();
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
        network: "testnet",
        modules: [setupMyNearWallet()],
      });
      const wallet = await selector.wallet("my-near-wallet");
      const accounts = await wallet.getAccounts();
      if (accounts.length !== 0) {
        setDeployerAccount(accounts[0]);
      }
    };
    fetchWalletAccounts();
  }, []);

  const addWalletAccount = async () => {
    const selector = await setupWalletSelector({
      // @ts-ignore: The 'import.meta' meta-property is only allowed when the '--module' option ...
      network: import.meta.env.VITE_NEAR_ENVIRONMENT ?? "testnet",
      modules: [setupMyNearWallet()],
    });
    const wallet: BrowserWallet = await selector.wallet("my-near-wallet");
    await wallet.signOut();
    wallet.signIn({ contractId: "calimero-package-manager.testnet" });
  };

  const handleFileChange = (event: ChangeEvent<HTMLInputElement>) => {
    const file = event.target.files && event.target.files[0];
    if (file && file.name.endsWith(".wasm")) {
      const reader = new FileReader();
      reader.onload = async (e) => {
        if (e && e.target && e.target.result) {
          const arrayBuffer = new Uint8Array(
            e.target.result as ArrayBufferLike
          );
          const bytes = new Uint8Array(arrayBuffer);
          const blob = new Blob([bytes], { type: "application/wasm" });

          const hashBuffer = await crypto.subtle.digest(
            "SHA-256",
            await blob.arrayBuffer()
          );

          const hashHex = Array.from(new Uint8Array(hashBuffer))
            .map((byte) => byte.toString(16).padStart(2, "0"))
            .join("");

          setFileHash(hashHex);

          await axios
            .post(BLOBBY_IPFS, blob)
            .then((response) => {
              setIpfsPath(`${BLOBBY_IPFS}/${response.data.cid}`);
            })
            .catch((error) => {
              console.error("Error occurred while uploading the file:", error);
            });
        } else {
          console.error("Failed to read file or file content is not available");
          return;
        }
      };

      reader.onerror = (e: ProgressEvent<FileReader>) => {
        console.error(
          "Error occurred while reading the file:",
          e.target?.error
        );
        return;
      };

      reader.readAsArrayBuffer(file);
    }
  };

  const addPackage = async () => {
    const selector = await setupWalletSelector({
      network: "testnet",
      modules: [setupMyNearWallet()],
    });
    const wallet = await selector.wallet("my-near-wallet");
    try {
      const res = await wallet.signAndSendTransaction({
        signerId: deployerAccount ? deployerAccount.accountId : "",
        actions: [
          {
            type: "FunctionCall",
            params: {
              methodName: "add_package",
              args: {
                name: packageInfo.name,
                description: packageInfo.description,
                repository: packageInfo.repository,
              },
              gas: nearAPI.utils.format.parseNearAmount("0.00000000003") ?? "0",
              deposit: "",
            },
          },
        ],
      });
      if (
        res &&
        isFinalExecutionStatus(res.status) &&
        res.status.SuccessValue
      ) {
        setDeployStatus({
          title: "Package added successfully",
          message: `Package ${packageInfo.name} added successfully`,
          error: false,
        });
      }
    } catch (error) {
      let errorMessage = "";

      if (error instanceof Error) {
        errorMessage =
          JSON.parse(error.message).kind?.kind?.FunctionCallError
            ?.ExecutionError ?? "An error occurred while publishing package";
      }

      setDeployStatus({
        title: "Failed to add package",
        message: errorMessage,
        error: true,
      });
    }
  };

  const addRelease = async () => {
    const selector = await setupWalletSelector({
      network: "testnet",
      modules: [setupMyNearWallet()],
    });
    const wallet = await selector.wallet("my-near-wallet");
    try {
      const res = await wallet.signAndSendTransaction({
        signerId: deployerAccount ? deployerAccount.accountId : "",
        actions: [
          {
            type: "FunctionCall",
            params: {
              methodName: "add_release",
              args: {
                name: packageInfo.name,
                version: releaseInfo.version,
                notes: releaseInfo.notes,
                path: releaseInfo.path,
                hash: releaseInfo.hash,
              },
              gas: nearAPI.utils.format.parseNearAmount("0.00000000003") ?? "0",
              deposit: "",
            },
          },
        ],
      });
      if (
        res &&
        isFinalExecutionStatus(res.status) &&
        res.status.SuccessValue === ""
      ) {
        setDeployStatus({
          title: "Application published",
          message: `Application ${packageInfo.name} with release version ${releaseInfo.version} published`,
          error: false,
        });
      }
    } catch (error) {
      let errorMessage = "";

      if (error instanceof Error) {
        errorMessage =
          JSON.parse(error.message).kind?.kind?.FunctionCallError
            ?.ExecutionError ??
          "An error occurred while publishing the release";
      }

      setDeployStatus({
        title: "Failed to add release",
        message: errorMessage,
        error: true,
      });
    }
  };

  const closeStatusModal = () => {
    setShowStatusModal(false);
    if (!deployStatus.error) {
      setPackageInfo({
        name: "",
        description: "",
        repository: "",
      });
      setReleaseInfo({
        name: "",
        version: "",
        notes: "",
        path: "",
        hash: "",
      });
      setFileHash("");
      setIpfsPath("");
      if (fileInputRef.current) {
        fileInputRef.current.value = "";
      }
    }

    setDeployStatus({
      title: "",
      message: "",
      error: false,
    });
  };

  const publishApplication = async () => {
    setIsLoading(true);
    setShowStatusModal(false);
    await addPackage();
    await addRelease();
    setShowStatusModal(true);
    setIsLoading(false);
  };

  return (
    <FlexLayout>
      <Navigation />
      <PageContentWrapper isOverflow={true}>
        <PublishApplicationTable
          addWalletAccount={addWalletAccount}
          navigateToApplications={() => navigate("/applications")}
          deployerAccount={deployerAccount}
          showStatusModal={showStatusModal}
          closeModal={closeStatusModal}
          deployStatus={deployStatus}
          packageInfo={packageInfo}
          setPackageInfo={setPackageInfo}
          handleFileChange={handleFileChange}
          ipfsPath={ipfsPath}
          fileHash={fileHash}
          packages={packages}
          releaseInfo={releaseInfo}
          setReleaseInfo={setReleaseInfo}
          fileInputRef={fileInputRef}
          publishApplication={publishApplication}
          isLoading={isLoading}
        />
      </PageContentWrapper>
    </FlexLayout>
  );
}
