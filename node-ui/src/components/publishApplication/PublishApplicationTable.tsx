import React from "react";
import styled from "styled-components";
import translations from "../../constants/en.global.json";
import { ContentCard } from "../common/ContentCard";
import { Account } from "@near-wallet-selector/core";
import { PackageInfo, ReleaseInfo } from "../../pages/PublishApplication";
import { AddPackageForm } from "./AddPackageForm";
import { AddReleaseForm } from "./AddReleaseForm";
import { ConnectWalletAccountCard } from "./ConnectWalletAccountCard";
import StatusModal, { ModalContent } from "../common/StatusModal";
import Button from "../common/Button";

const FlexWrapper = styled.div`
  flex: 1;

  .button-wrapper {
    padding: 1.5rem 1rem 2.563rem;
  }
`;

interface PublishApplicationTableProps {
  addWalletAccount: () => void;
  navigateToApplications: () => void;
  deployerAccount: Account | undefined;
  showStatusModal: boolean;
  closeModal: () => void;
  deployStatus: ModalContent;
  packageInfo: PackageInfo;
  setPackageInfo: React.Dispatch<React.SetStateAction<PackageInfo>>;
  handleFileChange: (e: React.ChangeEvent<HTMLInputElement>) => void;
  ipfsPath: string;
  fileHash: string;
  packages: PackageInfo[];
  releaseInfo: ReleaseInfo;
  setReleaseInfo: React.Dispatch<React.SetStateAction<ReleaseInfo>>;
  fileInputRef: React.RefObject<HTMLInputElement>;
  publishApplication: () => void;
  isLoading: boolean;
}
export default function PublishApplicationTable({
  addWalletAccount,
  navigateToApplications,
  deployerAccount,
  showStatusModal,
  closeModal,
  deployStatus,
  packageInfo,
  setPackageInfo,
  handleFileChange,
  fileHash,
  releaseInfo,
  setReleaseInfo,
  fileInputRef,
  publishApplication,
  isLoading
}: PublishApplicationTableProps) {
  const t = translations.applicationsPage.publishApplication;

  return (
    <ContentCard
      headerBackText={t.title}
      headerOnBackClick={navigateToApplications}
    >
      <StatusModal
        closeModal={closeModal}
        show={showStatusModal}
        modalContent={deployStatus}
      />
      <FlexWrapper>
        <AddPackageForm
          packageInfo={packageInfo}
          setPackageInfo={setPackageInfo}
        />
        <AddReleaseForm
          handleFileChange={handleFileChange}
          fileHash={fileHash}
          releaseInfo={releaseInfo}
          setReleaseInfo={setReleaseInfo}
          fileInputRef={fileInputRef}
        />
        <ConnectWalletAccountCard onClick={addWalletAccount} />
        <div className="button-wrapper">
          <Button
            text="Publish"
            width="100%"
            onClick={publishApplication}
            isDisabled={
              !(
                deployerAccount &&
                packageInfo.name &&
                packageInfo.description &&
                packageInfo.repository &&
                releaseInfo.version &&
                releaseInfo.notes &&
                releaseInfo.path &&
                releaseInfo.hash
              )
            }
            isLoading={isLoading}
          />
        </div>
      </FlexWrapper>
    </ContentCard>
  );
}
  
