import { useState } from "react";
import React from "react";
import Button from "react-bootstrap/Button";
import Dropdown from "react-bootstrap/Dropdown";
import * as nearAPI from "near-api-js";
import { Navigation } from "../components/Navigation";
import styled from "styled-components";

const LayoutWrapper = styled.div`
  display: flex;
  background-color: #121216;
  .content {
    padding-left: 26px;
    padding-right: 26px;
    padding-top: 48px;
    padding-bottom: 26px;
    width: 100%;
  }
  .content-card {
    background-color: #353540;
    border-radius: 4px;
    width: 100%;
    height: 100%;
  }
  .title {
    color: #fff;
  }
`;

export default function Applications() {
  const [selectedPackage, setSelectedPackage] = useState("");
  const [packages, setPackages] = useState([]);
  const [releases, setReleases] = useState([]);

  const provider = new nearAPI.providers.JsonRpcProvider(
    `https://rpc.testnet.near.org`
  );

  if (!packages.length) {
    (async () => { setPackages(await getPackages()); })();
  }

  return (
    <LayoutWrapper>
      <Navigation />
      <div className="content">
        <div className="content-card">
          <h1 className="title">Applications</h1>
          <Dropdown>
            <Dropdown.Toggle variant="success" id="dropdown-basic">
              Pick application
            </Dropdown.Toggle>
            <Dropdown.Menu>
            {packages.map((pkg) => (
              <Dropdown.Item onClick={async () => {
                setSelectedPackage(pkg.name);
                setReleases(await getReleases(pkg.name));
              }}>
                {pkg.name}
              </Dropdown.Item>
            ))}
            </Dropdown.Menu>
          </Dropdown>
          {
            releases.map((release) => (
              <div>
                <h3>{release.version}</h3>
                <p>{release.description}</p>
              </div>
            ))
          }
          <br></br>
          <Button
            onClick={() => installApplication(selectedPackage)}
            variant="primary"
          >
            Install
          </Button>{" "}
        </div>
      </div>
    </LayoutWrapper>
  );
}

const getPackages = async () => {
  const provider = new nearAPI.providers.JsonRpcProvider(
    `https://rpc.testnet.near.org`
  );

  const rawResult = await provider.query({
    request_type: "call_function",
    account_id: "calimero-package-manager.testnet",
    method_name: "get_packages",
    args_base64: btoa(
      JSON.stringify({
        offset: 0,
        limit: 10,
      })
    ),
    finality: "optimistic",
  });
  
  return JSON.parse(Buffer.from(rawResult.result).toString());
}

const getReleases = async (packageName) => {
  const provider = new nearAPI.providers.JsonRpcProvider(
    `https://rpc.testnet.near.org`
  );

  const rawResult = await provider.query({
    request_type: "call_function",
    account_id: "calimero-package-manager.testnet",
    method_name: "get_releases",
    args_base64: btoa(
      JSON.stringify({
        name: packageName,
        offset: 0,
        limit: 10,
      })
    ),
    finality: "optimistic",
  });
  
  return JSON.parse(Buffer.from(rawResult.result).toString());
}

const installApplication = async (selectedPackage) => {
  console.log(selectedPackage);
};