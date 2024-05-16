import React, { useState } from "react";
import styled from "styled-components";
import { AddNewItem } from "../common/AddNewItem";
import { ChevronUpDownIcon } from "@heroicons/react/24/solid";
import translations from "../../constants/en.global.json";
import MenuIconDropdown from "../common/MenuIconDropdown";
import DidEditor from "./DidEditor";
import { RootKey } from "src/pages/Identity";

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

  .item-no-apps {
    grid-column: span 11;
    font-size: 16px;
    color: #fff;
  }

  .header,
  .application-item {
    padding-top: 10px;
    padding-bottom: 10px;
    padding-left: 14px;
    display: grid;
    grid-template-columns: repeat(10, 1fr);
    grid-template-rows: auto;
    color: rbg(255, 255, 255, 0.7);
    font-size: 12px;
    gap: 24px;
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
  .item-type {
    color: rgb(255, 255, 255, 0.7);
    font-size: 14px;
  }

  .item-type {
    grid-column: span 2;
  }

  .item-id {
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

const enable_option = false;

interface IdentityTableProps {
  identityList: RootKey[];
  deleteIdentity: (id: number) => void;
  addIdentity: () => void;
}

export function IdentityTable({ identityList, deleteIdentity, addIdentity }: IdentityTableProps) {
  const [expandDid, setExpandDid] = useState(-1);
  const [didValue, setDidValue] = useState("");
  const t = translations.identityTable;

  return (
    <Table>
      {identityList.length === 0 ? (
        <div className="header">
          <p className="item-no-apps">{t.noIdentityText}</p>
        </div>
      ) : (
        <div className="header">
          <div className="item-type">{t.headerTypeText}</div>
          <div className="item-id">{t.headerPkText}</div>
        </div>
      )}
      {identityList && (
        <div className="scroll-list">
          {identityList?.map((identity, id: number) => {
            return (
              <div className="application-item" key={identity.signing_key}>
                <div className="item-type app-item">
                  {identity.signing_key.split(":")[0]}
                </div>
                <div className="item-id app-item">{`${identity.signing_key
                  .split(":")[1]
                  .substring(0, 4)}...${identity.signing_key
                  .split(":")[1]
                  .substring(
                    identity.signing_key.split(":")[1].length - 4,
                    identity.signing_key.split(":")[1].length
                  )}`}</div>
                {enable_option && (
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
                      options={[
                        {
                          title: t.deleteButtonText,
                          onClick: () => deleteIdentity(id),
                        },
                      ]}
                    />
                  </div>
                )}
                {enable_option && expandDid === id && (
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
