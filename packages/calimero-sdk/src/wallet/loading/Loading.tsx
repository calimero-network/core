import React, { Fragment } from "react";

export const Loading: React.FC = () => {
  const css = `
    .lds-dual-ring {
        color: #FF7A00;
    }
    .lds-dual-ring,
    .lds-dual-ring:after {
    box-sizing: border-box;
    }
    .lds-dual-ring {
    display: inline-block;
    width: 40px;
    height: 40px;
    }
    .lds-dual-ring:after {
    content: " ";
    display: block;
    width: 32px;
    height: 32px;
    margin: 8px;
    border-radius: 50%;
    border: 6.4px solid currentColor;
    border-color: currentColor transparent currentColor transparent;
    animation: lds-dual-ring 1.2s linear infinite;
    }
    @keyframes lds-dual-ring {
        0% {
            transform: rotate(0deg);
        }
        100% {
            transform: rotate(360deg);
        }
    }
    `;

  return (
    <Fragment>
      <style>{css}</style>
      <div className="lds-dual-ring"></div>
    </Fragment>
  );
};
