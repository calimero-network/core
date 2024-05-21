import React from "react";
import styled from "styled-components";
import { ButtonLight } from "../common/ButtonLight";

export const ContentLayout = styled.div`
  padding: 42px 26px 26px 26px;
  display: flex;
  flex: 1;
  .content-card {
    background-color: #353540;
    border-radius: 4px;
    padding: 30px 26px 26px 30px;
    width: 100%;
  }

  .page-title {
    color: #fff;
    font-size: 24px;
    font-weight: semi-bold;
  }
  .card-header {
    display: flex;
    justify-content: space-between;
    align-items: center;
  }
`;

interface ApplicationsContentProps {
  children: React.ReactNode;
  redirectAppUpload: () => void;
}

export function ApplicationsContent(props: ApplicationsContentProps) {
  return (
    <ContentLayout>
      <div className="content-card">
        <div className="card-header">
          <div className="page-title">Applications</div>
          <ButtonLight text="Upload Application" onClick={props.redirectAppUpload} />
        </div>
        {props.children}
      </div>
    </ContentLayout>
  );
}
