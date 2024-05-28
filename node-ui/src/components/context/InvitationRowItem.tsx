import React from "react";
import styled from "styled-components";
import { Invitation } from "../../pages/Contexts";
import Button from "../common/Button";
import { formatDateWithTime } from "../../utils/date";
import translations from "../../constants/en.global.json";

const RowItem = styled.div`
  display: flex;
  align-items: center;
  justify-content: space-between;
  background-color: #212325;
  min-height: 6.125rem;
  border-radius: 0.5rem;
  padding: 1rem;
  margin-left: 1rem;
  margin-right: 1rem;
  margin-bottom: 1rem;

  .flex-wrapper-w {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
  }

  .flex-wrapper-h {
    display: flex;
    justify-content: center;
    align-items: center;
    gap: 0.5rem;
  }

  .invitation-id {
    font-size: 0.875rem;
    font-weight: 500;
    line-height: 1.25rem;
    text-align: left;
    word-break: break-word;
  }

  .invitation-date {
    font-size: 0.875rem;
    font-weight: 400;
    line-height: 1.25rem;
    text-align: left;
    color: #a9a9a9;
  }
`;

export default function invitationRowItem(
  item: Invitation,
  id: number,
  _count: number,
  onitemClicked?: (id: string, isAccepted?: boolean) => void
): JSX.Element {
  const t = translations.contextPage.contextInvitation;
  return (
    <RowItem key={id}>
      <div className="flex-wrapper-w">
        <div className="invitation-id">{item.id}</div>
        <div className="invitation-date">
          {t.invitationText}
          {formatDateWithTime(item.invitedOn)}{" "}
        </div>
      </div>
      <div className="flex-wrapper-h">
        <Button
          onClick={() => onitemClicked && onitemClicked(item.id, true)}
          text={t.acceptButtonText}
          color="#6CECAC"
          disabledColor="#6CECAC"
          highlightColor="#6CECAC"
          fontSize="0.75rem"
          lineHeight="1rem"
          height="1.875rem"
          padding="0.4375rem 0.6875rem"
          width="3.938rem"
        />
        <Button
          onClick={() => onitemClicked && onitemClicked(item.id, false)}
          text={t.declineButtonText}
          color="transparent"
          disabledColor="transparent"
          highlightColor="transparent"
          textColor="#EF4444"
          fontSize="0.75rem"
          lineHeight="1rem"
          height="1.875rem"
          padding="0.4375rem 0.6875rem"
          width="3.938rem"
        />
      </div>
    </RowItem>
  );
}
