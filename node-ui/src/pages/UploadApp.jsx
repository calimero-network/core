import React from "react";
import { Navigation } from "../components/Navigation";
import { FlexLayout } from "../components/layout/FlexLayout";
import { useUploadFile } from "../hooks/useUploadFile";
import { UploadAppContent } from "../components/uploadApp/UploadAppContent";
import { UploadApplication } from "../components/uploadApp/UploadApplication";
import { useState } from "react";

export default function UploadApp() {
  const { cidString, commitWasm, fetchWasm } = useUploadFile();
  const [wasmFile, setWasmFile] = useState();
  const [isUploaded, setIsUploaded] = useState(false);
  const [downloadUrl, setDownloadUrl] = useState();

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
    setIsUploaded(false);
    try {
      await commitWasm(wasmFile);
      setIsUploaded(true);
    } catch (e) {
      console.log(e);
    }
  };

  const createDownloadUrl = async () => {
    let fileObject = await fetchWasm();
    setDownloadUrl(fileObject);
  };

  return (
    <FlexLayout>
      <Navigation />
      <UploadAppContent>
        <UploadApplication
          handleFileChange={handleFileChange}
          handleFileUpload={handleFileUpload}
          createDownloadUrl={createDownloadUrl}
          wasmFile={wasmFile}
          cidString={cidString}
          isUploaded={isUploaded}
          downloadUrl={downloadUrl}
        />
      </UploadAppContent>
    </FlexLayout>
  );
}
