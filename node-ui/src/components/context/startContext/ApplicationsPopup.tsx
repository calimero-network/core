import React, { useState, useEffect } from "react";
import Modal from "react-bootstrap/Modal";
import styled from "styled-components";
import { Options } from "../../../constants/ApplicationsConstants";
import ApplicationsTable from "./ApplicationsTable";
import { useRPC } from "../../../hooks/useNear";
import { Application, Package } from "../../../pages/Applications";
import { TableOptions } from "../../../components/common/OptionsHeader";
import { ContextApplication } from "../../../pages/StartContext";

const ModalWrapper = styled.div`
  background-color: #212325;
  border-radius: 0.5rem;
`;

const initialOptions = [
  {
    name: "Available",
    id: Options.AVAILABLE,
    count: 0,
  },
  {
    name: "Owned",
    id: Options.OWNED,
    count: 0,
  },
];

interface ApplicationsPopupProps {
  show: boolean;
  closeModal: () => void;
  setApplication: (application: ContextApplication) => void;
}

export interface Applications {
  available: Application[];
  owned: Application[];
}

export default function ApplicationsPopup({
  show,
  closeModal,
  setApplication,
}: ApplicationsPopupProps) {
  const { getPackages, getLatestRelease, getPackage } = useRPC();
  const [currentOption, setCurrentOption] = useState<string>(Options.AVAILABLE);
  const [tableOptions, setTableOptions] =
    useState<TableOptions[]>(initialOptions);
  const [applicationsList, setApplicationsList] = useState<Applications>({
    available: [],
    owned: [],
  });

  useEffect(() => {
    const setApplications = async () => {
      const packages = await getPackages();
      if (packages.length !== 0) {
        const tempApplications = await Promise.all(
          packages.map(async (appPackage: Package) => {
            const releseData = await getLatestRelease(appPackage.id);
            return { ...appPackage, version: releseData?.version! };
          })
        );
        setApplicationsList((prevState) => ({
          ...prevState,
          available: tempApplications,
        }));
        setTableOptions([
          {
            name: "Available",
            id: Options.AVAILABLE,
            count: tempApplications.length,
          },
          {
            name: "Owned",
            id: Options.OWNED,
            count: 0,
          },
        ]);
      }
    };
    setApplications();
  }, []);

  const selectApplication = async (applicationId: string) => {
    const application = await getPackage(applicationId);
    const release = await getLatestRelease(applicationId);
    setApplication({
      appId: applicationId,
      name: application.name,
      version: release?.version ?? "",
    });
    closeModal();
  };

  return (
    <Modal
      show={show}
      backdrop="static"
      keyboard={false}
      className="modal-xl"
      centered
    >
      <ModalWrapper>
        <ApplicationsTable
          applicationsList={applicationsList}
          currentOption={currentOption}
          setCurrentOption={setCurrentOption}
          tableOptions={tableOptions}
          closeModal={closeModal}
          selectApplication={selectApplication}
        />
      </ModalWrapper>
    </Modal>
  );
}
