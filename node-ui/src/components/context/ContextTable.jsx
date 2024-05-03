import React from "react";
import PropTypes from "prop-types";
import translations from "../../constants/en.global.json";
import { ContentCard } from "../common/ConentCard";
import { Content } from "../../constants/ContextConstants";

export default function ContextTable({
  pageContent,
  nodeContextList,
  switchContent,
}) {
  const t = translations.contextPage;
  return (
    <>
      {pageContent === Content.CONTEXT_LIST ? (
        <ContentCard
          headerTitle={t.contextPageTitle}
          headerOptionText={t.startNewContextText}
          headerOnOptionClick={() => switchContent(Content.START_NEW_CONTEXT)}
          headerDescription={t.contextPageDescription}
        >
          {nodeContextList?.map((context, index) => (
            <p key={index}>{context}</p>
          ))}
        </ContentCard>
      ) : (
        <ContentCard
        headerBackText={t.startNewContextText}
        headerOnBackClick={() => switchContent(Content.CONTEXT_LIST)}
        >
        </ContentCard>
      )}
    </>
  );
}

ContextTable.propTypes = {
  nodeContextList: PropTypes.array.isRequired,
  pageContent: PropTypes.string.isRequired,
  switchContent: PropTypes.func.isRequired,
};
