import React, { useEffect, useState } from "react";
import { Navigation } from "../components/Navigation";
import { FlexLayout } from "../components/layout/FlexLayout";
import { IdentityContent } from "../components/identity/IdentityContent";
import { useNavigate } from "react-router-dom";
import axios from "axios";

export default function Identity() {
  const navigate = useNavigate();
  const [rootKeys, setRootKeys] = useState([]);
  useEffect(() => {
    const setDids = async () => {
      const response = await axios.get("/admin-api/did");
      setRootKeys(response.data.data.root_keys);
    };
    setDids();
  }, []);

  return (
    <FlexLayout>
      <Navigation />
      <IdentityContent
        identityList={rootKeys}
        deleteIdentity={() => console.log("TODO")}
        addIdentity={() => navigate("/")}
      />
    </FlexLayout>
  );
}
