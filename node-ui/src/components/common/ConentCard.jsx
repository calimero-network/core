import React from "react";
import styled from "styled-components";
import PropTypes from "prop-types";
import { ArrowLeftIcon } from "@heroicons/react/24/solid";
import Button from "./Button";

const Container = styled.div`
  display: flex;
  flex: 1;
  flex-direction: column;
  gap: 1rem;
  height: 100%;
  padding: 2rem;
  border-radius: 0.5rem;
  background-color: #212325;
  color: #fff;

  .header-back {
    display: flex;
    justify-content: flex-start;
    align-items: center;
    height: 1.75rem;
    margin-top: 0.125rem;
    gap: 1rem;
    color: #fff;
    font-size: 1rem;
    font-weight: 500;
    line-height: 1.25rem;

    .arrow-icon-left {
      height: 1.5rem;
      width: 1.75rem;
      cursor: pointer;
    }
  }

  .main-wrapper {
    display: flex;
    flex: 1;
    background-color: #17191b;
    border-radius: 0.5rem;
  }

  .header-option {
    display: flex;
    flex-direction: column;
    margin-top: 0.125rem;
    gap: 1rem;

    .flex-wrapper {
      display: flex;
      flex: 1;
      justify-content: space-between;
      align-items: center;

      .title {
        font-size: 1rem;
        font-weight: 500;
        line-height: 1.25rem;
        text-align: left;
      }
    }

    .description {
      font-size: 0.875rem;
      font-weight: 500;
      line-height: 1.25rem;
      text-align: left;
      color: #6b7280;
    }
  }
`;

export function ContentCard({
  headerTitle,
  headerOptionText,
  headerOnOptionClick,
  headerDescription,
  headerBackText,
  headerOnBackClick,
  children,
}) {
  return (
    <Container>
      {headerBackText && (
        <div className="header-back">
          {headerOnBackClick && (
            <ArrowLeftIcon
              className="arrow-icon-left"
              onClick={headerOnBackClick}
            />
          )}
          {headerBackText}
        </div>
      )}
      {headerTitle && (
        <div className="header-option">
          <div className="flex-wrapper">
            <div className="title">{headerTitle}</div>
            {headerOptionText && headerOnOptionClick && (
              <Button onClick={headerOnOptionClick} text={headerOptionText} />
            )}
          </div>
          {headerDescription && (
            <div className="description">{headerDescription}</div>
          )}
        </div>
      )}
      <div className="main-wrapper">{children}</div>
    </Container>
  );
}

ContentCard.propTypes = {
  headerTitle: PropTypes.string,
  headerOptionText: PropTypes.string,
  headerOnOptionClick: PropTypes.func,
  headerDescription: PropTypes.string,
  headerBackText: PropTypes.string,
  headerOnBackClick: PropTypes.func,
  children: PropTypes.node.isRequired,
};
