import React from "react";
import styled from "styled-components";
import { Tooltip } from "react-tooltip";
import { AddNewItem } from "../common/AddNewItem";
import PropTypes from "prop-types";
import translations from "../../constants/en.global.json";
import MenuIconDropdown from "../common/MenuIconDropdown";

const Table = styled.div`
  height: 100%;
  display: flex;
  flex: 1;
  flex-direction: column;
  padding-left: 24px;

  .header {
    margin-bottom: 4px;
    margin-top: 12px;
  }

  .header,
  .application-item {
    padding-top: 10px;
    padding-bottom: 10px;
    padding-left: 14px;
    display: grid;
    gap: 24px;
    grid-template-columns: repeat(11, 1fr);
    grid-template-rows: auto;
    color: rbg(255, 255, 255, 0.7);
  }

  .scroll-list {
    flex: 1;
    overflow-y: auto;
    scrollbar-width: none;
    -ms-overflow-style: none;
    &::-webkit-scrollbar {
      display: none;
    }
    margin-bottom: 16px;
  }

  .item-no-apps {
    grid-column: span 11;
    text-font: 16px;
    color: #fff;
  }

  .item-name {
    grid-column: span 2;
  }

  .item-desc,
  .item-repo {
    grid-column: span 4;
  }

  .item-header {
    color: rgb(255, 255, 255, 0.7);
    font-size: 14px;
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
    text-decoration: none;
    color: #ff842d;
  }

  .add-new-wrapper {
    width: 100%;
    display: flex;
    justify-content: center;
    align-items: top;
    height: 40px;
  }

  .menu {
    grid-column: span 1;
    display: flex;
    justify-content: end;
  }
`;

export function ApplicationsTable({ applications, install, uninstall }) {
  const t = translations.applicationsPage.applicationsTable;

  return (
    <Table>
      {applications && applications.length === 0 ? (
        <div className="header">
          <p className="item-no-apps">{t.noApplicationsText}</p>
        </div>
      ) : (
        <div className="header">
          <div className="item-name item-header">{t.headerNameText}</div>
          <div className="item-desc item-header">{t.headerDescText}</div>
          <div className="item-repo item-header">{t.headerRepoText}</div>
        </div>
      )}
      {applications && (
        <div className="scroll-list">
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
                  data-tooltip-id={`my-tooltip-${id}`}
                >
                  {application.description}
                  {application.description.length > 52 && (
                    <Tooltip
                      id={`my-tooltip-${id}`}
                      content={application.description}
                    />
                  )}
                </div>
                <a
                  href={application.repository}
                  target="_blank"
                  className="app-item item-repo app-item-repo"
                >
                  {application.repository}
                </a>
                {false && (
                  <div className="menu">
                    <MenuIconDropdown
                      options={[
                        {
                          buttonText: t.uninstallButtonText,
                          onClick: () => uninstall(id),
                        },
                      ]}
                    />
                  </div>
                )}
              </div>
            );
          })}
        </div>
      )}
      <div className="add-new-wrapper">
        <AddNewItem text={t.installButtonText} onClick={install} />
      </div>
    </Table>
  );
}

ApplicationsTable.propTypes = {
  applications: PropTypes.array.isRequired,
  install: PropTypes.func.isRequired,
  uninstall: PropTypes.func.isRequired,
};
