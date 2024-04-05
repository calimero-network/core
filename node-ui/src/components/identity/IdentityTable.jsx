import React, { useState } from "react";
import styled from "styled-components";
import { Tooltip } from "react-tooltip";
import { AddNewItem } from "../common/AddNewItem";
import { ChevronUpDownIcon } from "@heroicons/react/24/solid";
import PropTypes from "prop-types";
import translations from "../../constants/en.global.json";
import MenuIconDropdown from "../common/MenuIconDropdown";
import DidEditor from "./DidEditor";

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
    grid-template-columns: repeat(12, 1fr);
    grid-template-rows: auto;
    color: rbg(255, 255, 255, 0.7);
    gap: 24px;
    font-size: 12px;
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
  .background-item {
    background-color: rgb(0, 0, 0, 0.12);
  }

  .grid-item {
    background-color: lightblue;
    padding: 20px;
    border: 1px solid #333;
  }

  .item-id,
  .item-type,
  .item-pk {
    color: rgb(255, 255, 255, 0.7);
    font-size: 14px;
  }

  .item-id,
  .item-type {
    grid-column: span 2;
  }

  .item-pk {
    grid-column: span 6;
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
  .menu {
    position: relative;
    grid-column: span 2;
    display: flex;
    justify-content: end;
    gap: 24px;
  }
  .expand-icon {
    height: 20px;
    width: 20px;
    cursor: pointer;
    color: #fff;
  }
  .add-new-wrapper {
    width: 100%;
    display: flex;
    justify-content: center;
    align-items: top;
    height: 40px;
  }
`;

export function IdentityTable({ identityList, deleteIdentity, addIdentity }) {
  const [expandDid, setExpandDid] = useState(-1);
  const [didValue, setDidValue] = useState("");
  const t = translations.identityTable;

  return (
    <Table>
      {identityList && (
        <div className="header">
          <div className="item-id">{t.headerIdText}</div>
          <div className="item-type">{t.headerTypeText}</div>
          <div className="item-pk">{t.headerPkText}</div>
        </div>
      )}
      {identityList && (
        <div className="scroll-list">
          {identityList?.map((identity, id) => {
            return (
              <div className="application-item" key={identity.id}>
                <div className="item-id app-item">{`${identity.id
                  .split(":")[2]
                  .substring(0, 4)}...${identity.id
                  .split(":")[2]
                  .substring(
                    identity.id.split(":")[2].length - 4,
                    identity.id.split(":")[2].length
                  )}`}</div>
                <div className="item-type app-item">
                  {identity.verificationMethod[0].type}
                </div>
                <div
                  className="item-pk app-item app-item-desc"
                  data-tooltip-id={`my-tooltip-${identity.id}`}
                >
                  {identity.verificationMethod[0].publicKeyMultibase}
                  {identity.verificationMethod[0].publicKeyMultibase.length >
                    52 && (
                    <Tooltip
                      id={`my-tooltip-${identity.id}`}
                      content={
                        identity.verificationMethod[0].publicKeyMultibase
                      }
                    />
                  )}
                </div>
                <div className="menu">
                  <ChevronUpDownIcon
                    className="expand-icon"
                    onClick={() => {
                      if (expandDid === id) {
                        setExpandDid(-1);
                      } else {
                        setExpandDid(id);
                        setDidValue(JSON.stringify(identity, null, 2));
                      }
                    }}
                  />
                  <MenuIconDropdown
                    onClick={() => deleteIdentity(id)}
                    buttonText={t.deleteButtonText}
                  />
                </div>
                {expandDid === id && (
                  <DidEditor
                    labelText={t.expandEditorTitle}
                    cancelText={t.cancelButtonText}
                    didValue={didValue}
                    saveText={t.saveButtonText}
                    setDidValue={setDidValue}
                    setExpandDid={setExpandDid}
                  />
                )}
              </div>
            );
          })}
        </div>
      )}
      <div className="add-new-wrapper">
        <AddNewItem text={t.addButtonText} onClick={addIdentity} />
      </div>
    </Table>
  );
}

IdentityTable.propTypes = {
  identityList: PropTypes.array.isRequired,
  deleteIdentity: PropTypes.func.isRequired,
  addIdentity: PropTypes.func.isRequired,
};
