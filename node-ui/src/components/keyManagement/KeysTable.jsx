import React from "react";
import styled from "styled-components";
import { AddNewItem } from "../common/AddNewItem";
import PropTypes from "prop-types";
import translations from "../../constants/en.global.json";
import MenuIconDropdown from "../common/MenuIconDropdown";
import { truncatePublicKey, getStatus } from "../../utils/displayFunctions";

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
  .item-wrapper {
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

  .item-pk,
  .item-type {
    grid-column: span 2;
  }

  ,
  .item-badge {
    display: flex;
    justify-content: end;
    align-items: center;
  }

  .item-header {
    color: rgb(255, 255, 255, 0.7);
    font-size: 14px;
  }

  .app-item {
    color: #fff;
    font-size: 14px;
  }

  .app-item-type,
  .app-item-badge {
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }

  .app-item-badge {
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
    grid-column: span 7;
    display: flex;
    gap: 16px;
    justify-content: end;
  }
  .badge {
    width: 5rem;
    padding: 4px 10px;
    text-align: center;
    border-radius: 12px;
    text-transform: capitalize;
  }
  .active {
    background-color: rgb(61, 210, 139, 0.1);
    color: #3dd28b;
  }
  .revoked {
    background-color: rgb(218, 73, 63, 0.1);
    color: #da493f;
  }
  .no-keys-container {
    font-size: 14px;
    color: #fff;
    padding-top: 12px;
    padding-bottom: 24px;
  }
`;

export function KeysTable({ nodeKeys, setActive, revokeKey, optionsEnabled }) {
  const t = translations.keysTable;
  return (
    <Table>
      {nodeKeys.length > 0 ? (
        <>
          <div className="header">
            <div className="item-pk item-header">{t.headerPkText}</div>
            <div className="item-type item-header">{t.headerTypeText}</div>
          </div>
          <div className="scroll-list">
            {nodeKeys?.map((key, id) => {
              return (
                <div className="item-wrapper" key={id}>
                  <div className="item-pk app-item">
                    {truncatePublicKey(key.publicKey)}
                  </div>
                  <div className="item-type app-item app-item-type">
                    {key.publicKey.split(":")[0]}
                  </div>
                  <div className="menu">
                    <div className="item-badge app-item">
                      <div
                        className={`badge ${getStatus(
                          key.active,
                          key.revoked
                        )}`}
                      >
                        {getStatus(key.active, key.revoked)}
                      </div>
                    </div>
                    {optionsEnabled && (
                      <MenuIconDropdown
                        options={[
                          {
                            buttonText: t.setActiveText,
                            onClick: () => setActive(id),
                          },
                          {
                            buttonText: t.revokeKeyText,
                            onClick: () => revokeKey(id),
                          },
                        ]}
                      />
                    )}
                  </div>
                </div>
              );
            })}
          </div>
        </>
      ) : (
        <div className="no-keys-container">{t.noKeysText}</div>
      )}
      <div className="add-new-wrapper">
        <AddNewItem
          text={t.addNewText}
          onClick={() => console.log("add new key")}
        />
      </div>
    </Table>
  );
}

KeysTable.propTypes = {
  nodeKeys: PropTypes.array.isRequired,
  setActive: PropTypes.func.isRequired,
  revokeKey: PropTypes.func.isRequired,
  optionsEnabled: PropTypes.bool.isRequired,
};
