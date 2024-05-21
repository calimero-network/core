import React from "react";
import styled from "styled-components";
import Dropdown from "react-bootstrap/Dropdown";
import { PackageItem } from "./Item";
import { Form } from "react-bootstrap";
import { ReleaseItem } from "./ReleaseItem";
import StatusModal, { ModalContent } from "../common/StatusModal";
import translations from "../../constants/en.global.json";
import { Package, Tabs, Release } from "../../pages/Applications";
import { ContentCard } from "../common/ContentCard";

const InstallApplicationForm = styled.div`
  position: relative;
  padding: 2rem;
  flex: 1;

  .title {
    display: flex;
    flex: 1;
    font-size: 1rem;
    line-height: 1.25rem;
    color: #fff;
    margin-bottom: 1rem;
  }
  .label {
    font-size: 0.75rem;
    color: rgb(255, 255, 255, 0.7);
  }

  .app-dropdown {
    background-color: #4cfafc;
    border: none;
    outline: none;
    color: #111;
    font-size: 0.875rem;;
    font-weight: normal;
    width: 15.625rem;
  }

  .dropdown-menu {
    background-color: #17171d;
    width: 15.625rem;
    max-height: 15.625rem;
    overflow: scroll;
  }

  .dropdown-item {
    color: #fff;
  }

  .dropdown-item:hover {
    background-color: rgb(255, 255, 255, 0.06);
  }

  .install-button {
    border-radius: 0.25rem;
    background-color: rgba(255, 255, 255, 0.06);
    width: fit-content;
    padding: 0.5rem 2rem;
    position: absolute;
    bottom: 1.5rem;
    right: 1.5rem;
    cursor: pointer;
    border: none;
    outline: none;
  }
  .install-button:hover {
    background-color: rgba(255, 255, 255, 0.12);
  }
  .release-item {
    margin-bottom: 0.25rem;
  }
  .radio-item {
    display: inline-block;
    margin-right: 0.625rem;
    padding-left: 0rem;
    width: 8.75rem;
    height: 1.5rem;
    margin-bottom: 0.5rem;
  }

  .radio-item input[type="radio"] {
    display: none;
  }

  .radio-item label {
    display: flex;
    justify-content: center;
    align-items: center;
    cursor: pointer;
    background-color: #17171d;
    color: white;
    border-radius: 0.375rem;
    padding: 0.25rem;
  }

  .radio-item input[type="radio"]:checked + label {
    background-color: #4cfafc;
    color: #111;
  }

  .radio-item label:hover {
    background-color: #2c2c33;
  }
`;

interface InstallApplicationProps {
  packages: Package[];
  releases: Release[];
  installApplication: () => void;
  setSelectedPackage: (installPackage: Package | null) => void;
  setReleases: (releases: Release[]) => void;
  getReleases: (pkgId: string) => Promise<Release[]>;
  selectedPackage: Package | null;
  selectedRelease: Release | null;
  setSelectedRelease: (release: Release | null) => void;
  setSelectedTab: (tab: Tabs) => void;
  showStatusModal: boolean;
  closeModal: () => void;
  installationStatus: ModalContent;
}

export function InstallApplication({
  packages,
  releases,
  installApplication,
  setSelectedPackage,
  setReleases,
  getReleases,
  selectedPackage,
  selectedRelease,
  setSelectedRelease,
  setSelectedTab,
  showStatusModal,
  closeModal,
  installationStatus,
}: InstallApplicationProps) {
  const t = translations.applicationsPage.installApplication;
  return (
    <ContentCard
      headerBackText={t.backbuttonText}
      headerOnBackClick={() => {
        setSelectedTab(Tabs.APPLICATION_LIST);
        setSelectedPackage(null);
        setSelectedRelease(null);
      }}
    >
      <StatusModal
        show={showStatusModal}
        closeModal={closeModal}
        modalContent={installationStatus}
      />
      <InstallApplicationForm>
        <div className="title">{t.title}</div>
        <label className="label">{t.selectAppLabel}</label>
        <Dropdown>
          <Dropdown.Toggle className="app-dropdown">
            {selectedPackage ? selectedPackage.name : t.dropdownPlaceholder}
          </Dropdown.Toggle>
          <Dropdown.Menu className="dropdown-menu">
            {packages.map((pkg, id) => (
              <Dropdown.Item
                onClick={async () => {
                  setSelectedPackage(pkg);
                  setSelectedRelease(null);
                  setReleases(await getReleases(pkg.id));
                }}
                key={id}
                className="dropdown-item"
              >
                {pkg.name}
              </Dropdown.Item>
            ))}
          </Dropdown.Menu>
        </Dropdown>
        {selectedPackage && (
          <>
            <label className="label">{t.packageDetailsLabel}</label>
            <PackageItem selectedItem={selectedPackage} />
            <label className="label">{t.releaseSelectionLabel}</label>
            <Form>
              <Form.Group>
                {releases.map((release, id) => {
                  return (
                    <div className="release-item" key={id}>
                      <Form.Check
                        type="radio"
                        label={release.version}
                        name="releaseRadio"
                        id={`releaseRadio-${id}`}
                        key={id}
                        checked={selectedRelease === release}
                        onChange={() => {
                          setSelectedRelease(release);
                        }}
                        className="radio-item"
                      />
                    </div>
                  );
                })}
              </Form.Group>
            </Form>
            {selectedRelease && (
              <>
                <label className="label">{t.releaseDetailsLabel}</label>
                <ReleaseItem release={selectedRelease} />
              </>
            )}
          </>
        )}
        <button
          className="install-button"
          onClick={installApplication}
          disabled={!selectedPackage || !selectedRelease}
        >
          Install
        </button>
      </InstallApplicationForm>
    </ContentCard>
  );
}
