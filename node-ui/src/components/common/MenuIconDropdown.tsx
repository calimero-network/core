import React from "react";
import { EllipsisVerticalIcon } from "@heroicons/react/24/solid";
import Dropdown from "react-bootstrap/Dropdown";
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
      background-color: #2D2D2D;
      display: flex;
      flex-direction: column;
      justify-content: start;
      padding: 8px 0 8px 0px;
      gap: 4px;
      border-radius: 4px;

      .menu-item {
        cursor: pointer;
        color: #9C9DA3;
        font-size: 14px;
        font-family: "Inter", sans-serif;
        font-optical-sizing: auto;
        font-weight: 400;
        font-style: normal;
        font-variation-settings: "slnt" 0;
        -webkit-font-smoothing: antialiased;
        -moz-osx-font-smoothing: grayscale;
        font-smooth: never;

        &:hover {
          background-color: transparent;
          color: #fff;
        }
      }
    }
  }
`;

interface Option {
  buttonText: string;
  onClick: () => void;
}

interface MenuIconDropdownProps {
  options: Option[];
}

export default function MenuIconDropdown({ options }: MenuIconDropdownProps) {
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
