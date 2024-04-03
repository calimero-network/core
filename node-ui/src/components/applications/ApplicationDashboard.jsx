import React from "react";
import styled from "styled-components";
import { Navigation } from "../Navigation";
import { ApplicationsTable } from "./ApplicationsTable";

const LayoutWrapper = styled.div`
  width: 100%;
  background-color: #121216;
`;

export function ApplicationDashboard() {
  return (
    <LayoutWrapper>
      <Navigation />
      <ApplicationsTable />
    </LayoutWrapper>
  );
}
