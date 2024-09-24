import React from 'react';
import { styled } from 'styled-components';

interface LoaderComponentProps {
  $loaderColor: string;
  $loaderSize: string;
  $borderSize: string;
}

const Loader = styled.span<LoaderComponentProps>`
    width: ${(props) => props.$loaderSize};
    height: ${(props) => props.$loaderSize};
    border: ${(props) => props.$borderSize} solid #FFF;
    border-bottom-color: ${(props) => props.$loaderColor};
    border-radius: 50%;
    display: inline-block;
    box-sizing: border-box;
    animation: rotation 1s linear infinite;
    }

    @keyframes rotation {
    0% {
        transform: rotate(0deg);
    }
    100% {
        transform: rotate(360deg);
    }
`;

interface LoadingProps {
  loaderColor?: string;
  loaderSize?: string;
  borderSize?: string;
}

export default function Loading({
  loaderColor = '#111',
  loaderSize = '20px',
  borderSize = '2px',
}: LoadingProps) {
  return (
    <Loader
      $loaderColor={loaderColor}
      $loaderSize={loaderSize}
      $borderSize={borderSize}
    ></Loader>
  );
}
