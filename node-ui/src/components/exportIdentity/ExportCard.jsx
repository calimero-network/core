import React from "react";
import styled from "styled-components";
import PropTypes from "prop-types";
import translations from "../../constants/en.global.json";
import Button from "../common/Button";
import StatusModalItem from "../common/StatusModalItem";

const CardWrapper = styled.div`
  padding: 2rem 2rem 3.75rem;
  height: fit-content;
  flex: 1;
  background-color: #212325;
  border-radius: 0.5rem;
  display: flex;
  flex-direction: column;
  gap: 1rem;

  .title {
    font-size: 1rem;
    font-weight: 500;
    line-height: 1.25rem;
    text-align: left;
    color: #fff;
  }

  .description {
    font-size: 0.875rem;
    font-weight: 500;
    line-height: 1.25rem;
    text-align: left;
    color: #6b7280;
  }
`;

const exportItem = (data) => {
    return (
        <div className="item">
            <div className="item-title">{data}</div>
        </div>
    );
}

export default function ExportCard({
  onClick,
  showStatusModal,
  closeStatusModal,
  exportStatus,
}) {
  const t = translations.exportIdentityPage;
  return (
    <CardWrapper>
      <StatusModalItem
        show={showStatusModal}
        closeModal={closeStatusModal}
        modalContent={exportStatus}
        itemObject={exportItem}
      />
      <div className="title">{t.title}</div>
      <div className="description">{t.description}</div>
      <Button text={t.buttonExportText} onClick={onClick} width={"182px"} />
    </CardWrapper>
  );
}

ExportCard.propTypes = {
  onClick: PropTypes.func.isRequired,
  showStatusModal: PropTypes.bool.isRequired,
  closeStatusModal: PropTypes.func.isRequired,
  exportStatus: PropTypes.object.isRequired,
};
