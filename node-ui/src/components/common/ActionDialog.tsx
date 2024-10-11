import React from 'react';
import Modal from 'react-bootstrap/Modal';
import styled from 'styled-components';
import translations from '../../constants/en.global.json';

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
    padding-left: 12px;
    text-align: start;
    color: #fff;
    font-size: 20px;
    font-weight: semi-bold;
  }

  .container {
    margin-top: 20px;

    .modal-subtitle {
      text-align: start;
      width: 100%;
      font-size: 14px;
      color: rgb(255, 255, 255, 0.7);
    }

    .button-wrapper {
      display: flex;
      justify-content: end;
      gap: 1rem;
      width: 100%;
      margin-top: 12px;

      .button-cancel {
        color: #111;
        background-color: #6cecac;
      }

      .button,
      .button-cancel {
        border-radius: 4px;
        width: fit-content;
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

      .button {
        background-color: #ef4444;
        color: #fff;
      }

      .buttons-wrapper {
        display: flex;
        justify-content: space-between;
      }
    }
  }
`;

interface ActionDialogProps {
  show: boolean;
  closeDialog: () => void;
  onConfirm: () => void;
  title: string;
  subtitle: string;
  buttonActionText?: string;
}

export default function ActionDialog({
  show,
  closeDialog,
  onConfirm,
  title,
  subtitle,
  buttonActionText = 'Delete',
}: ActionDialogProps) {
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
        <div className="modal-title">{title}</div>
        <div className="container">
          <div className="modal-subtitle">{subtitle}</div>
          <div className="button-wrapper">
            <button className="button-cancel" onClick={closeDialog}>
              {t.buttonCancelText}
            </button>
            <button className="button" onClick={onConfirm}>
              {buttonActionText}
            </button>
          </div>
        </div>
      </ModalWrapper>
    </Modal>
  );
}
