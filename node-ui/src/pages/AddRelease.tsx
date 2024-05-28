import React, { useState, useEffect, useRef, ChangeEvent } from "react";
import { Navigation } from "../components/Navigation";
import { FlexLayout } from "../components/layout/FlexLayout";
import {
  Account,
  BrowserWallet,
  FinalExecutionOutcome,
  NetworkId,
  setupWalletSelector,
} from "@near-wallet-selector/core";
import { setupMyNearWallet } from "@near-wallet-selector/my-near-wallet";
import { useRPC } from "../hooks/useNear";
import axios from "axios";

import * as nearAPI from "near-api-js";
import { Package } from "./Applications";
import PageContentWrapper from "../components/common/PageContentWrapper";
import { useNavigate, useParams } from "react-router-dom";
import AddReleaseTable from "../components/publishApplication/addRelease/AddReleaseTable";
import { DeployStatus, ReleaseInfo } from "./PublishApplication";
import { isFinalExecutionStatus } from "../utils/wallet";

const BLOBBY_IPFS = "https://blobby-public.euw3.prod.gcp.calimero.network";

export default function AddRelease() {
  const { id } = useParams();
  const navigate = useNavigate();
  const fileInputRef = useRef<HTMLInputElement | null>(null);
  const [ipfsPath, setIpfsPath] = useState("");
  const [fileHash, setFileHash] = useState("");
  const { getPackage, getLatestRelease } = useRPC();
  const [deployerAccount, setDeployerAccount] = useState<Account>();
  const [showStatusModal, setShowStatusModal] = useState(false);
  const [isLoading, setIsLoading] = useState(false);
  const [applicationInformation, setApplicationInformation] =
    useState<Package>();
  const [latestRelease, setLatestRelease] = useState("");
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
    const fetchPackageInfo = async () => {
      if (id) {
        const packageInfo = await getPackage(id);
        setApplicationInformation(packageInfo);
        const latestRelease = await getLatestRelease(id);
        setLatestRelease(latestRelease?.version!);
      }
    };

    fetchWalletAccounts();
    fetchPackageInfo();
  }, [id]);

  const addWalletAccount = async () => {
    const selector = await setupWalletSelector({
      network: process.env["VITE_NEAR_ENVIRONMENT"] as NetworkId || "testnet",
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
      reader.onload = async (e: ProgressEvent<FileReader>) => {
        const arrayBuffer = new Uint8Array(e.target?.result as ArrayBufferLike);
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
      };

      reader.onerror = (e: ProgressEvent<FileReader>) => {
        console.error(
          "Error occurred while reading the file:",
          e.target?.error
        );
      };

      reader.readAsArrayBuffer(file);
    }
  };

  const addRelease = async () => {
    const selector = await setupWalletSelector({
      network: "testnet",
      modules: [setupMyNearWallet()],
    });
    const wallet = await selector.wallet("my-near-wallet");
    try {
      const res: FinalExecutionOutcome | void =
        await wallet.signAndSendTransaction({
          signerId: deployerAccount ? deployerAccount.accountId : "",
          actions: [
            {
              type: "FunctionCall",
              params: {
                methodName: "add_release",
                args: {
                  name: applicationInformation?.name!,
                  version: releaseInfo.version,
                  notes: releaseInfo.notes,
                  path: releaseInfo.path,
                  hash: releaseInfo.hash,
                },
                gas:
                  nearAPI.utils.format.parseNearAmount("0.00000000003") ?? "0",
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
          message: `Release version ${
            releaseInfo.version
          } for ${applicationInformation?.name!} has been added!`,
          error: false,
        });
      }
    } catch (error) {
      let errorMessage = "";

      if (error instanceof Error) {
        errorMessage = JSON.parse(error.message).kind?.kind?.FunctionCallError
        ?.ExecutionError ?? "An error occurred while publishing the release";
      }

      setDeployStatus({
        title: "Failed to publish release",
        message: errorMessage,
        error: true,
      });
    }
  };

  const closeStatusModal = () => {
    setShowStatusModal(false);
    if (!deployStatus.error) {
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

  const publishRelease = async () => {
    setIsLoading(true);
    setShowStatusModal(false);
    await addRelease();
    setShowStatusModal(true);
    setIsLoading(false);
  };

  return (
    <FlexLayout>
      <Navigation />
      <PageContentWrapper isOverflow={true}>
        <AddReleaseTable
          addWalletAccount={addWalletAccount}
          navigateToApplicationDetails={() => navigate(`/applications/${id}`)}
          deployerAccount={deployerAccount}
          showStatusModal={showStatusModal}
          closeModal={closeStatusModal}
          deployStatus={deployStatus}
          applicationInformation={applicationInformation}
          latestRelease={latestRelease}
          handleFileChange={handleFileChange}
          ipfsPath={ipfsPath}
          fileHash={fileHash}
          releaseInfo={releaseInfo}
          setReleaseInfo={setReleaseInfo}
          fileInputRef={fileInputRef}
          publishRelease={publishRelease}
          isLoading={isLoading}
        />
      </PageContentWrapper>
    </FlexLayout>
  );
}
