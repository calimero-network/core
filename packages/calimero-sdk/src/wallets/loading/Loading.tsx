import React from "react";

export const Loading: React.FC = () => {
  const css = `
  .loader {
    width: 48px;
    height: 48px;
    border: 5px solid #FFF;
    border-bottom-color: #FF7A00;
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
  }
  `;

  return (
    <>
      <style>{css}</style>
      <span className="loader"></span>
    </>
  );
};
