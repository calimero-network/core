import React, { useState, useEffect, useRef } from "react";
import { Navigation } from "../components/Navigation";
import { FlexLayout } from "../components/layout/FlexLayout";
import { UploadAppContent } from "../components/uploadApp/UploadAppContent";
import { UploadApplication } from "../components/uploadApp/UploadApplication";
import { AddPackageForm } from "../components/uploadApp/AddPackageForm";
import { UploadSwitch } from "../components/uploadApp/UploadSwitch";
import { setupWalletSelector } from "@near-wallet-selector/core";
import { setupMyNearWallet } from "@near-wallet-selector/my-near-wallet";
import { useRPC } from "../hooks/useNear";
import axios from "axios";

import * as nearAPI from "near-api-js";

const BLOBBY_IPFS = "https://blobby-public.euw3.prod.gcp.calimero.network";

export default function UploadApp() {
  const fileInputRef = useRef(null);
  const { getPackages } = useRPC();
  const [ipfsPath, setIpfsPath] = useState("");
  const [fileHash, setFileHash] = useState("");
  const [tabSwitch, setTabSwitch] = useState(true);
  const [packages, setPackages] = useState([]);
  const [addPackageLoader, setAddPackageLoader] = useState(false);
  const [addReleaseLoader, setAddReleaseLoader] = useState(false);
  const [walletAccounts, setWalletAccounts] = useState([]);
  const [deployerAccount, setDeployerAccount] = useState(null);
  const [showStatusModal, setShowStatusModal] = useState(false);
  const [packageInfo, setPackageInfo] = useState({
    name: "",
    description: "",
    repository: "",
  });
  const [deployStatus, setDeployStatus] = useState({
    title: "",
    message: "",
    error: false,
  });
  const [releaseInfo, setReleaseInfo] = useState({
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
      setWalletAccounts(accounts);
    };
    fetchWalletAccounts();
  }, []);

  const addWalletAccount = async () => {
    const selector = await setupWalletSelector({
      network: import.meta.env.VITE_NEAR_ENVIRONMENT ?? "testnet",
      modules: [setupMyNearWallet()],
    });
    const wallet = await selector.wallet("my-near-wallet");
    await wallet.signOut();
    wallet.signIn({ contractId: "calimero-package-manager.testnet" });
  };

  const handleFileChange = (event) => {
    const file = event.target.files[0];
    if (file && file.name.endsWith(".wasm")) {
      const reader = new FileReader();
      reader.onload = async (e) => {
        const arrayBuffer = new Uint8Array(e.target.result);
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

      reader.onerror = (e) => {
        console.error("Error occurred while reading the file:", e.target.error);
      };

      reader.readAsArrayBuffer(file);
    }
  };

  const addPackage = async (packageInfo) => {
    setAddPackageLoader(true);
    const selector = await setupWalletSelector({
      network: "testnet",
      modules: [setupMyNearWallet()],
    });
    const wallet = await selector.wallet("my-near-wallet");
    try {
      const res = await wallet.signAndSendTransaction({
        signerId: deployerAccount,
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
              gas: nearAPI.utils.format.parseNearAmount("0.00000000003"),
            },
          },
        ],
      });
      if (res.status.SuccessValue) {
        setDeployStatus({
          title: "Package added successfully",
          message: `Package ${packageInfo.name} added successfully`,
          error: false,
        });
      }
    } catch (error) {
      const errorMessage =
        JSON.parse(error.message).kind?.kind?.FunctionCallError
          ?.ExecutionError ?? "An error occurred while adding the package";

      setDeployStatus({
        title: "Failed to add Package",
        message: errorMessage,
        error: true,
      });
    }
    setShowStatusModal(true);
    setAddPackageLoader(false);
  };

  const addRelease = async (releaseInfo) => {
    setAddReleaseLoader(true);
    const selector = await setupWalletSelector({
      network: "testnet",
      modules: [setupMyNearWallet()],
    });
    const wallet = await selector.wallet("my-near-wallet");
    try {
      const res = await wallet.signAndSendTransaction({
        signerId: deployerAccount,
        actions: [
          {
            type: "FunctionCall",
            params: {
              methodName: "add_release",
              args: {
                name: releaseInfo.name,
                version: releaseInfo.version,
                notes: releaseInfo.notes,
                path: releaseInfo.path,
                hash: releaseInfo.hash,
              },
              gas: nearAPI.utils.format.parseNearAmount("0.00000000003"),
            },
          },
        ],
      });

      if (res.status.SuccessValue === '') {
        setDeployStatus({
          title: "Release added successfully",
          message: `Release version ${releaseInfo.version} for ${releaseInfo.name} added successfully`,
          error: false,
        });
      }
    } catch (error) {
      const errorMessage =
        JSON.parse(error.message).kind?.kind?.FunctionCallError
          ?.ExecutionError ?? "An error occurred while adding the release";

      setDeployStatus({
        title: "Failed to add Release",
        message: errorMessage,
        error: true,
      });
    }
    setShowStatusModal(true);
    setAddReleaseLoader(false);
  };

  const closeStatusModal = () => {
    setShowStatusModal(false);
    if (!deployStatus.error) {
      setDeployerAccount(null);
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


  return (
    <FlexLayout>
      <Navigation />
      <UploadAppContent addWalletAccount={addWalletAccount}>
        <UploadSwitch setTabSwitch={setTabSwitch} tabSwitch={tabSwitch}>
          {tabSwitch ? (
            <AddPackageForm
              addPackage={addPackage}
              addPackageLoader={addPackageLoader}
              walletAccounts={walletAccounts}
              deployerAccount={deployerAccount}
              setDeployerAccount={setDeployerAccount}
              showStatusModal={showStatusModal}
              closeModal={closeStatusModal}
              deployStatus={deployStatus}
              packageInfo={packageInfo}
              setPackageInfo={setPackageInfo}
            />
          ) : (
            <UploadApplication
              handleFileChange={handleFileChange}
              addRelease={addRelease}
              ipfsPath={ipfsPath}
              fileHash={fileHash}
              packages={packages}
              walletAccounts={walletAccounts}
              deployerAccount={deployerAccount}
              setDeployerAccount={setDeployerAccount}
              showStatusModal={showStatusModal}
              addReleaseLoader={addReleaseLoader}
              closeModal={closeStatusModal}
              deployStatus={deployStatus}
              releaseInfo={releaseInfo}
              setReleaseInfo={setReleaseInfo}
              fileInputRef={fileInputRef}
            />
          )}
        </UploadSwitch>
      </UploadAppContent>
    </FlexLayout>
  );
}
