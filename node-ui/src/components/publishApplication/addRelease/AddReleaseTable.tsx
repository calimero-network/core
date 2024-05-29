import React from "react";
import styled from "styled-components";
import translations from "../../../constants/en.global.json";
import { ContentCard } from "../../common/ContentCard";
import { Account } from "@near-wallet-selector/core";
import { ReleaseInfo } from "../../../pages/PublishApplication";
import { AddReleaseForm } from "../AddReleaseForm";
import { ConnectWalletAccountCard } from "../ConnectWalletAccountCard";
import StatusModal, { ModalContent } from "../../common/StatusModal";
import Button from "../../common/Button";
import { Package } from "../../../pages/Applications";
import DetailsCard from "../../applications/details/DetailsCard";

const FlexWrapper = styled.div`
  flex: 1;

  .button-wrapper {
    padding: 1.5rem 1rem 2.563rem;
  }

  .latest-version {
    padding: 1rem;
    font-size: 1rem;
    font-weight: 500;
    line-height: 1.25rem;
    color: #6b7280;
  }
`;

interface AddReleaseTableProps {
  addWalletAccount: () => void;
  navigateToApplicationDetails: () => void;
  deployerAccount: Account | undefined;
  showStatusModal: boolean;
  closeModal: () => void;
  deployStatus: ModalContent;
  applicationInformation: Package | undefined;
  latestRelease: string;
  handleFileChange: (e: React.ChangeEvent<HTMLInputElement>) => void;
  ipfsPath: string;
  fileHash: string;
  releaseInfo: ReleaseInfo;
  setReleaseInfo: React.Dispatch<React.SetStateAction<ReleaseInfo>>;
  fileInputRef: React.RefObject<HTMLInputElement>;
  publishRelease: () => void;
  isLoading: boolean;
}
export default function AddReleaseTable({
  addWalletAccount,
  navigateToApplicationDetails,
  deployerAccount,
  showStatusModal,
  closeModal,
  deployStatus,
  applicationInformation,
  latestRelease,
  handleFileChange,
  fileHash,
  releaseInfo,
  setReleaseInfo,
  fileInputRef,
  publishRelease,
  isLoading,
}: AddReleaseTableProps) {
  const t = translations.applicationsPage.addRelease;

  return (
    <ContentCard
      headerBackText={t.title}
      headerOnBackClick={navigateToApplicationDetails}
      descriptionComponent={
        applicationInformation && <DetailsCard details={applicationInformation} />
      }
      isOverflow={true}
    >
      <StatusModal
        closeModal={closeModal}
        show={showStatusModal}
        modalContent={deployStatus}
      />
      <FlexWrapper>
        <div className="latest-version">
          {t.latestReleaseText}
          {latestRelease}
        </div>
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
            text={t.buttonText}
            width="100%"
            onClick={publishRelease}
            isDisabled={
              !(
                deployerAccount &&
                applicationInformation?.name &&
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
