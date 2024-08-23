import React from 'react';
import styled from 'styled-components';
import translations from '../../constants/en.global.json';

const FooterWrapper = styled.div`
  display: flex;
  justify-content: center;
  align-items: center;
  position: absolute;
  bottom: 0;
  width: 100%;
  padding-bottom: 1rem;

  .footer-text {
    margin: 0px 0px 0.35em;
    font-weight: 400;
    font-size: 14px;
    line-height: 21px;
    letter-spacing: 0px;
    text-transform: none;
    color: rgba(255, 255, 255, 0.7);
    text-decoration: none;
  }
`;

export function Footer() {
  const t = translations.footer;
  return (
    <FooterWrapper>
      <a
        href="https://www.calimero.network"
        className="footer-text"
        target="_blank"
        rel="noreferrer"
      >
        {t.title}
      </a>
    </FooterWrapper>
  );
}
