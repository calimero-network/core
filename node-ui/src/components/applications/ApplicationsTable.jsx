import React, { useState } from "react";
import styled from "styled-components";
import { Tooltip } from "react-tooltip";
import { AddNewItem } from "../addItem/AddNewItem";
import { EllipsisVerticalIcon } from "@heroicons/react/24/solid";
import PropTypes from "prop-types";
import translations from "../../constants/en.global.json";

const Table = styled.div`
  padding-left: 24px;

  .header {
    margin-bottom: 4px;
    margin-top: 32px;
  }

  .header,
  .application-item {
    padding-top: 10px;
    padding-bottom: 10px;
    padding-left: 14px;
    display: grid;
    grid-template-columns: repeat(11, 1fr);
    grid-template-rows: auto;
    gap: 40px;
    color: rbg(255, 255, 255, 0.7);
    font-size: 12px;
  }

  .background-item {
    background-color: rgb(0, 0, 0, 0.12);
  }

  .grid-item {
    background-color: lightblue;
    padding: 20px;
    border: 1px solid #333;
  }

  .item-name {
    grid-column: span 2;
  }

  .item-desc {
    grid-column: span 4;
  }

  .item-repo {
    grid-column: span 4;
  }

  .app-item {
    color: #fff;
    font-size: 14px;
  }

  .app-item-desc,
  .app-item-repo {
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .app-item-repo {
    font-size: 10px;
    text-decoration: none;
    color: #ff842d;
  }

  .button-wrapper {
    display: flex;
    width: 100%;
    justify-content: center;
    align-items: center;
    padding-top: 32px;
  }

  .menu {
    position: relative;
    grid-column: span 1;
    display: flex;
    justify-content: end;
  }
  .menu-icon {
    height: 20px;
    width: 20px;
    cursor: pointer;
  }

  .menu-popup {
    width: 120px;
    position: absolute;
    right: 0;
    top: 22px;
    z-index: 10;
    background-color: #17171d;
    display: flex;
    flex-direction: column;
    justify-content: start;
    padding-left: 14px;
    padding-top: 10px;
    padding-bottom: 10px;
    gap: 14px;
    border-radius: 4px;
  }
  .menu-item {
    cursor: pointer;
  }
`;

export function ApplicationsTable({ applications, install, uninstall }) {
  const [showMenuPopup, setShowMenuPopup] = useState(-1);
  const t = translations.applicationsTable;

  return (
    <Table>
      {applications && (
        <div className="header">
          <div className="item-name">{t.headerNameText}</div>
          <div className="item-desc">{t.headerDescText}</div>
          <div className="item-repo">{t.headerRepoText}</div>
        </div>
      )}
      {applications?.map((application, id) => {
        return (
          <div
            className={`application-item ${
              id % 2 === 1 ? "background-item" : ""
            }`}
            key={application.id}
          >
            <div className="item-name app-item">{application.name}</div>
            <div
              className="item-desc app-item app-item-desc"
              data-tooltip-id={`my-tooltip-${application.id}`}
            >
              {application.description}
              {application.description.length > 52 && (
                <Tooltip
                  id={`my-tooltip-${application.id}`}
                  content={application.description}
                />
              )}
            </div>
            <div className="item-repo">
              <a
                href={application.repository}
                target="_blank"
                className="app-item app-item-repo"
              >
                {application.repository}
              </a>
            </div>
            <div className="menu">
              <EllipsisVerticalIcon
                className="menu-icon"
                onClick={() =>
                  showMenuPopup === id
                    ? setShowMenuPopup(-1)
                    : setShowMenuPopup(id)
                }
              />
              {showMenuPopup === id && (
                <div className="menu-popup">
                  <div className="menu-item" onClick={uninstall}>
                    {t.uninstallButtonText}
                  </div>
                </div>
              )}
            </div>
          </div>
        );
      })}
      <div className="button-wrapper">
        <AddNewItem text={t.installButtonText} onClick={install} />
      </div>
    </Table>
  );
}

ApplicationsTable.propTypes = {
  applications: PropTypes.object.isRequired,
  install: PropTypes.func.isRequired,
  uninstall: PropTypes.func.isRequired,
};
