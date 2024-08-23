import React from 'react';
import styled from 'styled-components';
import Button from '../../common/Button';
import translations from '../../../constants/en.global.json';
import StatusModal, { ModalContent } from '../../common/StatusModal';
import { ArrowLeftIcon } from '@heroicons/react/24/solid';

const CardWrapper = styled.div`
  padding: 2rem 2rem 3.75rem;
  height: fit-content;
  flex: 1;
  background-color: #212325;
  border-radius: 0.5rem;
  display: flex;
  flex-direction: column;
  gap: 1rem;

  .title-wrapper {
    display: flex;
    gap: 0.5rem;

    .title {
      font-size: 1rem;
      font-weight: 500;
      line-height: 1.25rem;
      text-align: left;
      color: #fff;
    }

    .arrow-icon-left {
      height: 1.5rem;
      width: 1.75rem;
      cursor: pointer;
      color: #fff;
    }
  }

  .label {
    color: rgb(255, 255, 255, 0.4);
    font-size: 0.625rem;
    font-weight: 500;
    line-height: 0.75rem;
    text-align: left;
  }

  input {
    background-color: transparent;
    margin-bottom: 1rem;
    padding: 0.5rem;
    border: 1px solid rgb(255, 255, 255, 0.1);
    background-color: rgb(255, 255, 255, 0.2);
    border-radius: 0.25rem;
    font-size: 0.875rem;
    color: rgb(255, 255, 255, 0.7);
    outline: none;
    width: 60%;
  }

  .input:focus {
    border: 1px solid #4cfafc;
  }
`;

interface JoinContextCardProps {
  handleJoinContext: () => void;
  contextId: string;
  setContextId: (contextId: string) => void;
  showModal: boolean;
  modalContent: ModalContent;
  closeModal: () => void;
  navigateBack: () => void;
}

export default function JoinContextCard({
  handleJoinContext,
  contextId,
  setContextId,
  showModal,
  modalContent,
  closeModal,
  navigateBack,
}: JoinContextCardProps) {
  const t = translations.joinContextPage;

  return (
    <CardWrapper>
      <StatusModal
        show={showModal}
        closeModal={closeModal}
        modalContent={modalContent}
      />
      <div className="title-wrapper">
        <ArrowLeftIcon className="arrow-icon-left" onClick={navigateBack} />
        <div className="title">{t.title}</div>
      </div>
      <label className="label">{t.contextIdLabel}</label>
      <input
        className="input"
        value={contextId}
        onChange={(e) => setContextId(e.target.value)}
      />
      <Button
        text={t.buttonJoinText}
        onClick={handleJoinContext}
        width={'11.375rem'}
        isDisabled={!contextId}
      />
    </CardWrapper>
  );
}
