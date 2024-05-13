import React from "react";
import { useLocation, useNavigate } from "react-router-dom";
import { RootKeyContainer } from "../components/confirmWallet/RootKeyContainer";
import { useState } from "react";
import { getParams, submitRootKeyRequest } from "../utils/rootkey";

export default function ConfirmWallet() {
  const location = useLocation();
  const navigate = useNavigate();
  const params = getParams(location);
  const [showStatusModal, setShowStatusModal] = useState(false);
  const [addRootKeyStatus, setAddRootKeyStatus] = useState({
    title: "",
    message: "",
    error: false,
  });

  const addRootKey = async () => {
    let addRootKeyResponse = await submitRootKeyRequest(params);

    if (addRootKeyResponse.error) {
      setAddRootKeyStatus({
        title: "Failed to add root key",
        message: addRootKeyResponse.error,
        error: true,
      });
    } else {
      setAddRootKeyStatus({
        title: "Success",
        message: addRootKeyResponse.data,
        error: false,
      });
    }
    setShowStatusModal(true);
  };

  const closeStatusModal = () => {
    setShowStatusModal(false);
    setAddRootKeyStatus({
      title: "",
      message: "",
      error: false,
    });
    if (!addRootKeyStatus.error) {
      navigate("/identity");
    }
  };

  return (
    <RootKeyContainer
      params={params}
      submitRootKeyRequest={addRootKey}
      showStatusModal={showStatusModal}
      closeStatusModal={closeStatusModal}
      addRootKeyStatus={addRootKeyStatus}
    />
  );
}
