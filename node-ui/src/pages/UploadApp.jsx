import React, { useState, useEffect } from "react";
import { Navigation } from "../components/Navigation";
import { FlexLayout } from "../components/layout/FlexLayout";
import { useUploadFile } from "../hooks/useUploadFile";
import { UploadAppContent } from "../components/uploadApp/UploadAppContent";
import { UploadApplication } from "../components/uploadApp/UploadApplication";
import { AddToContract } from "../components/uploadApp/AddToContract";
import { setupWalletSelector } from "@near-wallet-selector/core";
import { setupMyNearWallet } from "@near-wallet-selector/my-near-wallet";
import { useRPC } from "../hooks/useNear";

import * as nearAPI from "near-api-js";

export default function UploadApp() {
  const { cidString, commitWasm } = useUploadFile();
  const [wasmFile, setWasmFile] = useState();
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
      reader.onload = (e) => {
        const arrayBuffer = e.target.result;
        const bytes = new Uint8Array(arrayBuffer);
        setWasmFile(bytes);
      };

      reader.onerror = (e) => {
        console.error("Error occurred while reading the file:", e.target.error);
      };

      reader.readAsArrayBuffer(file);
    }
  };

  const handleFileUpload = async () => {
    try {
      await commitWasm(wasmFile);
    } catch (e) {
      console.log(e);
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
    if (res.status.SuccessValue === "") {
      setAddPackageLoader(false);
      window.alert("Package added successfully!");
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
            handleFileUpload={handleFileUpload}
            wasmFile={wasmFile}
            setTabSwitch={setTabSwitch}
            addRelease={addRelease}
            cidString={cidString}
            packages={packages}
            addReleaseLoader={addReleaseLoader}
          />
        ) : (
          <AddToContract
            cid={cidString}
            addPackage={addPackage}
            setTabSwitch={setTabSwitch}
            addPackageLoader={addPackageLoader}
          />
        )}
      </UploadAppContent>
    </FlexLayout>
  );
}
