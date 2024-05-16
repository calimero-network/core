import React from "react";
import Modal from "react-bootstrap/Modal";
import styled from "styled-components";
import {
  ExclamationTriangleIcon,
  ShieldCheckIcon,
} from "@heroicons/react/24/solid";
import translations from "../../constants/en.global.json";

const ModalWrapper = styled.div`
  display: flex;
  flex-direction: column;
  justify-content: center;
  padding: 16px;
  border-radius: 6px;
  items-align: center;
  background-color: #353540;
  text-align: center;

  .error-icon,
  .success-icon {
    height: 32px;
    width: 32px;
  }

  .error-icon {
    color: #da493f;
  }

  .success-icon {
    color: #3dd28b;
  }

  .modal-title {
    color: #fff;
    font-size: 20px;
    font-weight: semi-bold;
  }

  .container {
    margin-top: 20px;

    .modal-item {
      width: 100%;
      font-size: 14px;
      color: rgb(255, 255, 255, 0.7);
    }

    .button-wrapper {
      width: 100%;
      margin-top: 12px;

      .button {
        border-radius: 4px;
        background-color: #ff842d;
        color: #111;
        width: 100%;
        height: 30px;
        padding-left: 14px;
        padding-right: 14px;
        margin-top: 8px;
        cursor: pointer;
        border: none;
        outline: none;
        diplay: flex;
        justify-content: center;
        align-items: center;
      }

      .button:hover {
        background-color: #ac5221;
      }

      .buttons-wrapper {
        display: flex;
        justify-content: space-between;
      }
    }
  }
`;

export interface ModalContentItem {
  title: string;
  data: string;
  error: boolean;
}

interface StatusModalItemProps {
  show: boolean;
  closeModal: () => void;
  modalContent: ModalContentItem;
  itemObject: (data: string) => JSX.Element;
}

export default function StatusModalItem({
  show,
  closeModal,
  modalContent,
  itemObject,
}: StatusModalItemProps) {
  const t = translations.statusModal;
  return (
    <Modal
      show={show}
      backdrop="static"
      keyboard={false}
      aria-labelledby="contained-modal-title-vcenter"
      centered
    >
      <ModalWrapper>
        <div>
          {modalContent.error ? (
            <ExclamationTriangleIcon className="error-icon" />
          ) : (
            <ShieldCheckIcon className="success-icon" />
          )}
        </div>
        <div className="modal-title">{modalContent.title}</div>
        <div className="container">
          <div className="modal-item">{itemObject(modalContent.data)}</div>
          <div className="button-wrapper">
            <button className="button" onClick={closeModal}>
              {modalContent.error ? t.buttonCloseText : t.buttonCopyText}
            </button>
          </div>
        </div>
      </ModalWrapper>
    </Modal>
  );
}
