import React from 'react';
import styled from 'styled-components';
import { ArrowLeftIcon } from '@heroicons/react/24/solid';
import Button from './Button';

const Container = styled.div<{ $isOverflow: boolean }>`
  display: flex;
  flex: 1;
  flex-direction: column;
  gap: 1rem;
  padding: 2rem;
  border-radius: 0.5rem;
  background-color: #212325;
  color: #fff;
  ${(props) => (props.$isOverflow ? '' : 'height: 100%;')}

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

  .options-container {
    display: flex;
    gap: 1rem;
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

  .description-component {
    display: flex;
  }
`;

interface ContentCardProps {
  headerTitle?: string;
  headerOptionText?: string;
  headerOnOptionClick?: () => void;
  headerSecondOptionText?: string;
  headerOnSecondOptionClick?: () => void;
  headerDescription?: string;
  headerBackText?: string;
  headerOnBackClick?: () => void;
  children: React.ReactNode;
  descriptionComponent?: React.ReactNode;
  isOverflow?: boolean;
}

export function ContentCard({
  headerTitle,
  headerOptionText,
  headerOnOptionClick,
  headerSecondOptionText,
  headerOnSecondOptionClick,
  headerDescription,
  headerBackText,
  headerOnBackClick,
  children,
  descriptionComponent,
  isOverflow = false,
}: ContentCardProps) {
  return (
    <Container $isOverflow={isOverflow}>
      {(headerTitle || headerBackText) && (
        <div className="header-option">
          <div className="flex-wrapper">
            {headerTitle ? (
              <div className="title">{headerTitle}</div>
            ) : (
              <div className="header-back">
                {headerBackText && headerOnBackClick && (
                  <ArrowLeftIcon
                    className="arrow-icon-left"
                    onClick={headerOnBackClick}
                  />
                )}
                {headerBackText}
              </div>
            )}
            <div className="options-container">
              {headerOnSecondOptionClick && (
                <Button
                  onClick={headerOnSecondOptionClick}
                  text={headerSecondOptionText!}
                />
              )}
              {headerOnOptionClick && (
                <Button
                  onClick={headerOnOptionClick}
                  text={headerOptionText!}
                />
              )}
            </div>
          </div>
          {headerDescription && (
            <div className="description">{headerDescription}</div>
          )}
        </div>
      )}
      {descriptionComponent && (
        <div className="description-component">{descriptionComponent}</div>
      )}
      <div className="main-wrapper">{children}</div>
    </Container>
  );
}
