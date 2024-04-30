import React from "react";
import PropTypes from "prop-types";

export default function ContextTable({ nodeContextList }) {
  return (
    <div>
      {nodeContextList?.map((context, index) => (
        <p key={index}>{context}</p>
      ))}
    </div>
  );
}

ContextTable.propTypes = {
  nodeContextList: PropTypes.array.isRequired,
};
