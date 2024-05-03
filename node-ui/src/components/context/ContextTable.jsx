import React from "react";
import PropTypes from "prop-types";

import { ContentCard } from "../common/ConentCard";


export default function ContextTable({ nodeContextList }) {
  return (
    <ContentCard>
      {nodeContextList?.map((context, index) => (
        <p key={index}>{context}</p>
      ))}
    </ContentCard>
  );
}

ContextTable.propTypes = {
  nodeContextList: PropTypes.array.isRequired,
};
