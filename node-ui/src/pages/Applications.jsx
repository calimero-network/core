import { useState } from "react";
import React from "react";
import Button from 'react-bootstrap/Button';
import Dropdown from 'react-bootstrap/Dropdown';
import * as nearAPI from "near-api-js";

export default function Applications() {
    const location = useLocation();
    const [selectedItem, setSelectedItem] = useState("");

    const provider = new nearAPI.providers.JsonRpcProvider(
      `https://rpc.testnet.near.org`
    );

    (async () => {
      const rawResult = await provider.query({
        request_type: "call_function",
        account_id: "calimero-package-manager.testnet",
        method_name: "get_packages",
        args_base64: btoa(JSON.stringify({
          offset: 0,
          limit: 10,
        })),
        finality: "optimistic",
      });
      const res = JSON.parse(Buffer.from(rawResult.result).toString());
      console.log(res);
    })();

    return (
      <>
        <Navigation />
      <h1>Applications</h1>
      <Dropdown>
    <Dropdown.Toggle variant="success" id="dropdown-basic">
      Pick application
    </Dropdown.Toggle>
    <Dropdown.Menu>
      <Dropdown.Item onClick={() => setSelectedItem("kv-store")}>kv-store</Dropdown.Item>
      <Dropdown.Item onClick={() => setSelectedItem("only-peers")}>only-peers</Dropdown.Item>
    </Dropdown.Menu>
  </Dropdown>
  <br></br>
  <Button onClick={() => installApplication(selectedItem) } variant="primary">Install</Button>{' '}
      </>
  );
}

const installApplication = async (selectedItem) => {
  console.log(selectedItem);
}

const getPackages = async () => {

}
