import React from 'react';
import styled from 'styled-components';

const Button = styled.div`
  display: inline-flex;
  -webkit-box-align: center;
  align-items: center;
  -webkit-box-pack: center;
  justify-content: center;
  position: relative;
  box-sizing: border-box;
  -webkit-tap-highlight-color: transparent;
  outline: 0px;
  border: 0px;
  margin: 0px;
  cursor: pointer;
  user-select: none;
  vertical-align: middle;
  appearance: none;
  text-decoration: none;
  transition:
    background-color 250ms cubic-bezier(0.4, 0, 0.2, 1) 0ms,
    box-shadow 250ms cubic-bezier(0.4, 0, 0.2, 1) 0ms,
    border-color 250ms cubic-bezier(0.4, 0, 0.2, 1) 0ms,
    color 250ms cubic-bezier(0.4, 0, 0.2, 1) 0ms;
  background-color: rgb(255, 132, 45);
  box-shadow:
    rgba(0, 0, 0, 0.2) 0px 3px 1px -2px,
    rgba(0, 0, 0, 0.14) 0px 2px 2px 0px,
    rgba(0, 0, 0, 0.12) 0px 1px 5px 0px;
  min-width: 0px;
  border-radius: 4px;
  white-space: nowrap;
  color: rgb(23, 23, 29);
  font-weight: 400;
  font-size: 14px;
  padding-left: 10px;
  padding-right: 10px;
  letter-spacing: 0px;

  &:hover {
    background-color: #ac5221;
  }
`;

interface ButtonLightProps {
  text: string;
  onClick: () => void;
}
console.log("abc");
console.log("abc");

export function ButtonLight({ text, onClick }: ButtonLightProps) {
  return (
    <>
       <Button className="button" onClick={onClick}>
          <Button className="button" onClick={onClick}>
             <Button className="button" onClick={onClick}>
                <Button className="button" onClick={onClick}>
      {text}
    </Button>
      <div>hellooo</div>
    </>
   
  );
}
