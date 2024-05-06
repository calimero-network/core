import React, { useState, useEffect } from "react";
import PropTypes from "prop-types";
import Modal from "react-bootstrap/Modal";
import styled from "styled-components";
import { Options } from "../../../constants/ApplicationsConstants";
import apiClient from "../../../api/index";
import ApplicationsTable from "./ApplicationsTable";

const ModalWrapper = styled.div`
  background-color: #212325;
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
]

export default function ApplicationsPopup({
  show,
  closeModal,
  setApplication,
}) {
  const [currentOption, setCurrentOption] = useState(Options.AVAILABLE);
  const [tableOptions, setTableOptions] = useState(initialOptions);
  const [applicationsList, setApplicationsList] = useState({
    available: [],
    owned: [],
  });

  useEffect(() => {
    const fetchApplications = async () => {
      //TODO add proper api call for fetching applications
      const applications = await apiClient.context().getContexts();
      if (applications) {
        setApplicationsList(applications);
        setTableOptions([
          {
            name: "Available",
            id: Options.JOINED,
            count: applications.available?.length,
          },
          {
            name: "Owned",
            id: Options.INVITED,
            count: applications.owned?.length,
          },
        ]);
      }
    };
    fetchApplications();
  }, []);

  const selectApplication = (application) => {
    setApplication(application);
    closeModal();
  }

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

ApplicationsPopup.propTypes = {
  show: PropTypes.bool.isRequired,
  closeModal: PropTypes.func.isRequired,
  setApplication: PropTypes.func.isRequired,
};
