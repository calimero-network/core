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
      border-radius: 0.5rem;
    }

    .menu-icon:after {
      display: block;
    }

    .menu-icon:active,
    .menu-icon:focus {
      background: #76f5f9;
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
      gap: 4px;
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

export default function MenuIconDropdown({ options }) {
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
            {options.map((option, id) => (
              <Dropdown.Item
                className="menu-item"
                onClick={option.onClick}
                key={id}
              >
                {option.buttonText}
              </Dropdown.Item>
            ))}
          </div>
        </Dropdown.Menu>
      </Dropdown>
    </DropdownWrapper>
  );
}

MenuIconDropdown.propTypes = {
  options: PropTypes.array.isRequired,
};
