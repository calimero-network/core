import React from "react";
import { Navigation } from "../components/Navigation";
import { FlexLayout } from "../components/layout/FlexLayout";
import styled from "styled-components";

const ExportWrapper = styled.div`
  display: flex;
  align-items: center;
  justify-content: center;
  flex-direction: column;
  width: 100%;
  padding: 2rem;
  gap: 1rem;

  .card {
    background-color: #212325;
    border-radius: 0.5rem;
    padding: 1rem;
    width: 100%;
    max-width: 30rem;
    text-align: center;
    color: #fff;
  }
`;
export default function Export() {
  return (
    <FlexLayout>
      <Navigation />
      <ExportWrapper>
        <div className="card">Coming soon!</div>
      </ExportWrapper>
    </FlexLayout>
  );
}
