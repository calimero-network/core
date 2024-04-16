import React, { useState, useEffect } from "react";
import { Navigation } from "../components/Navigation";
import { FlexLayout } from "../components/layout/FlexLayout";
import { UploadAppContent } from "../components/uploadApp/UploadAppContent";
import { UploadApplication } from "../components/uploadApp/UploadApplication";
import { AddPackageForm } from "../components/uploadApp/AddPackageForm";
import { setupWalletSelector } from "@near-wallet-selector/core";
import { setupMyNearWallet } from "@near-wallet-selector/my-near-wallet";
import { useRPC } from "../hooks/useNear";
import axios from "axios";

import * as nearAPI from "near-api-js";

const BLOBBY_IPFS = "https://blobby-public.euw3.prod.gcp.calimero.network";

export default function UploadApp() {
  const [ipfsPath, setIpfsPath] = useState("");
  const [fileHash, setFileHash] = useState("");
  const [tabSwitch, setTabSwitch] = useState(false);
  const [packages, setPackages] = useState([]);
  const [addPackageLoader, setAddPackageLoader] = useState(false);
  const [addReleaseLoader, setAddReleaseLoader] = useState(false);
  const { getPackages } = useRPC();

  useEffect(() => {
    if (!packages.length) {
      (async () => {
        setPackages(await getPackages());
      })();
    }
  }, [packages]);

  const addWalletAccount = async () => {
    const selector = await setupWalletSelector({
      network: "testnet",
      modules: [setupMyNearWallet()],
    });
    const wallet = await selector.wallet("my-near-wallet");
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
        const hashArray = Array.from(new Uint8Array(hashBuffer));
        const hashHex = hashArray
          .map((byte) => ("00" + byte.toString(16)).slice(-2))
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
    const account = (await wallet.getAccounts())[0];
    const res = await wallet.signAndSendTransaction({
      signerId: account,
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
      setAddPackageLoader(false);
      window.alert("Package added successfully!");
      setTabSwitch(true);
    }
  };

  const addRelease = async (releaseInfo) => {
    setAddReleaseLoader(true);
    const selector = await setupWalletSelector({
      network: "testnet",
      modules: [setupMyNearWallet()],
    });
    const wallet = await selector.wallet("my-near-wallet");
    const account = (await wallet.getAccounts())[0];
    const res = await wallet.signAndSendTransaction({
      signerId: account,
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
    if (res.status.SuccessValue === "") {
      setAddReleaseLoader(false);
      window.alert("Release added successfully!");
    }
  };

  return (
    <FlexLayout>
      <Navigation />
      <UploadAppContent addWalletAccount={addWalletAccount}>
        {tabSwitch ? (
          <UploadApplication
            handleFileChange={handleFileChange}
            setTabSwitch={setTabSwitch}
            addRelease={addRelease}
            ipfsPath={ipfsPath}
            fileHash={fileHash}
            packages={packages}
            addReleaseLoader={addReleaseLoader}
          />
        ) : (
          <AddPackageForm
            cid={ipfsPath}
            addPackage={addPackage}
            setTabSwitch={setTabSwitch}
            addPackageLoader={addPackageLoader}
          />
        )}
      </UploadAppContent>
    </FlexLayout>
  );
}
