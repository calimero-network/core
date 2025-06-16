import React from 'react';
import styled from 'styled-components';
import translations from '../../constants/en.global.json';
import { ContentCard } from '../common/ContentCard';
import { Account } from '@near-wallet-selector/core';
import { PackageInfo, ReleaseInfo, DeployStatus } from '../../pages/PublishApplication';
import { AddPackageForm } from './AddPackageForm';
import { AddReleaseForm } from './AddReleaseForm';
import { ConnectWalletAccountCard } from './ConnectWalletAccountCard';
import StatusModal from '../common/StatusModal';
import Button from '../common/Button';

const FlexWrapper = styled.div`
  flex: 1;

  .button-wrapper {
    padding: 1.5rem 1rem 2.563rem;
  }
  padding: 1rem;
`;

interface PublishApplicationTableProps {
  packageInfo: PackageInfo;
  setPackageInfo: React.Dispatch<React.SetStateAction<PackageInfo>>;
  releaseInfo: ReleaseInfo;
  setReleaseInfo: React.Dispatch<React.SetStateAction<ReleaseInfo>>;
  fileInputRef: React.RefObject<HTMLInputElement>;
  handleFileChange: (e: React.ChangeEvent<HTMLInputElement>) => void;
  deployerAccount: Account | undefined;
  addWalletAccount: () => void;
  publishApplication: () => void;
  showStatusModal: boolean;
  closeStatusModal: () => void;
  deployStatus: DeployStatus;
  isLoading: boolean;
}

export default function PublishApplicationTable({
  packageInfo,
  setPackageInfo,
  releaseInfo,
  setReleaseInfo,
  fileInputRef,
  handleFileChange,
  deployerAccount,
  addWalletAccount,
  publishApplication,
  showStatusModal,
  closeStatusModal,
  deployStatus,
  isLoading,
}: PublishApplicationTableProps) {
  const t = translations.applicationsPage.publishApplication;

  return (
    <ContentCard
      headerBackText={t.title}
      headerOnBackClick={() => window.history.back()}
    >
      <StatusModal
        closeModal={closeStatusModal}
        show={showStatusModal}
        modalContent={deployStatus}
      />
      <FlexWrapper>
        <ConnectWalletAccountCard
          onClick={addWalletAccount}
          deployerAccount={deployerAccount?.accountId}
        />

        {deployerAccount && (
          <>
            <AddPackageForm
              packageInfo={packageInfo}
              setPackageInfo={setPackageInfo}
            />
            <AddReleaseForm
              handleFileChange={handleFileChange}
              fileHash={releaseInfo.hash}
              releaseInfo={releaseInfo}
              setReleaseInfo={setReleaseInfo}
              fileInputRef={fileInputRef}
            />
            <div className="button-wrapper">
              <Button
                text={t.buttonText}
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
          </>
        )}
      </FlexWrapper>
    </ContentCard>
  );
}
