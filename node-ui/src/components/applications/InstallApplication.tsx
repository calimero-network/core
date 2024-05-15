import React from "react";
import styled from "styled-components";
import Dropdown from "react-bootstrap/Dropdown";
import { ArrowLeftIcon } from "@heroicons/react/24/solid";
import { PackageItem } from "./Item";
import { Form } from "react-bootstrap";
import { ReleaseItem } from "./ReleaseItem";
import StatusModal, { ModalContent } from "../common/StatusModal";
import translations from "../../constants/en.global.json";
import { Package, Tabs, Release } from "../../pages/Applications";

const InstallApplicationForm = styled.div`
  color: #fff;
  position: relative;
  height: 100%;

  .back-button {
    width: fit-content;
    display: flex;
    justify-content: center;
    align-items: center;
    gap: 4px;
    font-size: 14px;
    cursor: pointer;
    color: rgb(255, 255, 255, 0.7);
    .arrow-icon {
      height: 18px;
      width: 18px;
    }
  }

  .install-form {
    padding: 12px;
    .label {
      font-size: 12px;
      color: rgb(255, 255, 255, 0.7);
    }
    .app-dropdown {
      background-color: #ff842d;
      border: none;
      outline: none;
      color: #111;
      font-size: 14px;
      font-weight: normal;
      width: 250px;
    }
    .dropdown-menu {
      background-color: #17171d;
      width: 250px;
    }
    .dropdown-item {
      color: #fff;
    }
    .dropdown-item:hover {
      background-color: rgb(255, 255, 255, 0.06);
    }
  }

  .install-button {
    border-radius: 4px;
    background-color: rgba(255, 255, 255, 0.06);
    width: fit-content;
    padding: 8px 32px 8px 32px;
    position: absolute;
    bottom: 24px;
    right: 0px;
    cursor: pointer;
    border: none;
    outline: none;
  }
  .install-button:hover {
    background-color: rgba(255, 255, 255, 0.12);
  }
  .release-item {
    margin-bottom: 4px;
  }
  .radio-item {
    display: inline-block;
    margin-right: 10px;
    padding-left: 0px;
    width: 140px;
    height: 24px;
    margin-bottom: 8px;
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
    border-radius: 5px;
    padding: 4px;
  }

  .radio-item input[type="radio"]:checked + label {
    background-color: #ff842d;
  }

  .radio-item label:hover {
    background-color: #2c2c33;
  }
`;

interface InstallApplicationProps {
  packages: Package[];
  releases: Release[];
  installApplication: () => void;
  setSelectedPackage: (pkg: Package) => void;
  setReleases: (releases: Release[]) => void;
  getReleases: (pkgId: string) => Promise<Release[]>;
  selectedPackage: Package | null;
  selectedRelease: Release | null;
  setSelectedRelease: (release: Release) => void;
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
    <InstallApplicationForm>
      <StatusModal
        show={showStatusModal}
        closeModal={closeModal}
        modalContent={installationStatus}
      />
      <div
        onClick={() => {
          setSelectedTab(Tabs.APPLICATION_LIST);
          setSelectedPackage(null);
          setSelectedRelease(null);
        }}
        className="back-button"
      >
        <ArrowLeftIcon className="arrow-icon" />
        {t.backbuttonText}
      </div>
      <div className="install-form">
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
      </div>
    </InstallApplicationForm>
  );
}
