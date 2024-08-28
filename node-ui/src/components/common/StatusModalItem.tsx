import React from 'react';
import Modal from 'react-bootstrap/Modal';
import styled from 'styled-components';
import {
  ExclamationTriangleIcon,
  ShieldCheckIcon,
} from '@heroicons/react/24/solid';
import translations from '../../constants/en.global.json';
import Button from './Button';

const ModalWrapper = styled.div`
  display: flex;
  flex-direction: column;
  justify-content: center;
  padding: 1rem;
  border-radius: 0.375rem;
  items-align: center;
  background-color: #17191b;
  text-align: center;

  .error-icon,
  .success-icon {
    height: 2rem;
    width: 2rem;
  }

  .error-icon {
    color: #da493f;
  }

  .success-icon {
    color: #3dd28b;
  }

  .modal-title {
    font-size: 1rem;
    font-weight: 500;
    line-height: 1.25rem;
    color: #fff;
  }

  .container {
    margin-top: 1.25rem;

    .modal-item {
      width: 100%;
      font-size: 0.875rem;
      color: rgb(255, 255, 255, 0.7);
    }

    .button-wrapper {
      width: 100%;
      margin-top: 0.75rem;
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
            <Button
              width="100%"
              text={modalContent.error ? t.buttonCloseText : t.buttonCopyText}
              onClick={closeModal}
            />
          </div>
        </div>
      </ModalWrapper>
    </Modal>
  );
}
