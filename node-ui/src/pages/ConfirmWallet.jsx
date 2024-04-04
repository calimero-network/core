import React from "react";
import { useLocation, useNavigate } from "react-router-dom";
import { RootKeyContainer } from "../components/confirmWallet/RootKeyContainer";
import { useEffect, useState } from "react";
import {
  getParams,
  submitRootKeyRequest,
  isRootKeyAdded,
} from "../utils/rootkey";

export default function ConfirmWallet() {
  const location = useLocation();
  const navigate = useNavigate();
  const params = getParams(location);
  const [rootkeyAdded, setRootKeyAdded] = useState(false);

  useEffect(() => {
    if (isRootKeyAdded() || rootkeyAdded) {
      navigate("/identity");
    }
  }, [rootkeyAdded, navigate]);

  const addRootKey = async () => {
    let data = submitRootKeyRequest(params);
    if (data) {
      setRootKeyAdded(true);
    }
  };

  return <RootKeyContainer params={params} submitRootKeyRequest={addRootKey} />;
}
