import React from "react";
import { ButtonLight } from "../common/ButtonLight";
import styled from "styled-components";
import PropTypes from "prop-types";

const EditorContainer = styled.div`
  position: relative;
  background-color: #17171d;
  grid-column: span 12;
  border-radius: 4px;
  padding-top: 10px;
  margin-left: 8px;
  margin-right: 8px;
  margin-top: 8px;
  .expand-editor {
    font-size: 12px;
    width: 100%;
    height: 200px;

    background-color: #17171d;
    border-radius: 4px;
    padding: 10px;
    outline: none;
    resize: none;
    border: none;
  }
  .label {
    width: 100%;
    color: rgb(255, 132, 45);
    font-size: 14px;
    padding-left: 12px;
    font-weight: semi-bold;
    width: 100%;
  }

  .button-container {
    position: absolute;
    bottom: 10px;
    right: 10px;
    display: flex;
    justify-content: end;
    gap: 12px;
    width: 100%;
  }
`;

export default function DidEditor({
  labelText,
  cancelText,
  saveText,
  didValue,
  setDidValue,
  setExpandDid,
}) {
  return (
    <EditorContainer>
      <label className="label">{labelText}</label>
      <textarea
        className="expand-editor"
        value={didValue}
        onChange={(e) => setDidValue(e.target.value)}
      ></textarea>
      <div className="button-container">
        <ButtonLight text={cancelText} onClick={() => setExpandDid(-1)} />
        <ButtonLight
          text={saveText}
          onClick={() => {
            setExpandDid(-1);
            console.log(didValue);
          }}
        />
      </div>
    </EditorContainer>
  );
}

DidEditor.propTypes = {
  labelText: PropTypes.string.isRequired,
  cancelText: PropTypes.string.isRequired,
  saveText: PropTypes.string.isRequired,
  didValue: PropTypes.string.isRequired,
  setDidValue: PropTypes.func.isRequired,
  setExpandDid: PropTypes.func.isRequired,
};
