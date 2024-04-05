import React from "react";
import { EllipsisVerticalIcon } from "@heroicons/react/24/solid";
import Dropdown from "react-bootstrap/Dropdown";
import PropTypes from "prop-types";
import styled from "styled-components";

const DropdownWrapper = styled.div`
  .app-dropdown {
    background-color: transparent;
    border: none;
    padding: 0;
    margin: 0;
    --bs-btn-font-size: 0;

    .menu-icon {
      height: 20px;
      width: 20px;
      cursor: pointer;
      color: #fff;
    }
  }
  .app-dropdown.dropdown-toggle::after {
    display: none;
  }
  .app-dropdown.dropdown-toggle:active {
    background-color: transparent;
    outline: none;
  }

  .dropdown-container {
    padding: 0;

    .menu-dropdown {
      width: 100%;
      height: 100%;
      background-color: #17171d;
      display: flex;
      flex-direction: column;
      justify-content: start;
      padding: 4px 0 4px 14px;
      gap: 14px;
      border-radius: 4px;

      .menu-item {
        cursor: pointer;
        color: #fff;
        font-size: 14px;
        &:hover {
          background-color: transparent;
        }
      }
    }
  }
`;

export default function MenuIconDropdown({ onClick, buttonText }) {
  return (
    <DropdownWrapper>
      <Dropdown>
        <Dropdown.Toggle
          className="app-dropdown dropdown-toggle"
          data-bs-toggle="dropdown"
          aria-expanded="false"
        >
          <EllipsisVerticalIcon className="menu-icon" />
        </Dropdown.Toggle>
        <Dropdown.Menu className="dropdown-container">
          <div className="menu-dropdown">
            <Dropdown.Item className="menu-item" onClick={onClick}>
              {buttonText}
            </Dropdown.Item>
          </div>
        </Dropdown.Menu>
      </Dropdown>
    </DropdownWrapper>
  );
}

MenuIconDropdown.propTypes = {
  onClick: PropTypes.func.isRequired,
  buttonText: PropTypes.string.isRequired,
};
