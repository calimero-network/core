import { useState } from "react";
import {
  FeelbackTaggedMessage,
  Question,
  PRESET_YESNO_LIKE_DISLIKE,
  PRESET_FEEDBACK,
} from "@feelback/react";
import "@feelback/react/styles/feelback.css";

const YES_TAGS = [
  {
    value: "accurate",
    title: "Accurate",
    description: "Accurately describes the product or feature.",
  },
  {
    value: "problem-solved",
    title: "Solved my problem",
    description: "Helped me resolve an issue.",
  },
  {
    value: "clear",
    title: "Easy to understand",
    description: "Easy to follow and comprehend.",
  },
  {
    value: "product-chosen",
    title: "Helped me decide to use the product",
    description: "Convinced me to adopt the product or feature.",
  },
  { value: "other-yes", title: "Another reason" },
];

const NO_TAGS = [
  {
    value: "inaccurate",
    title: "Inaccurate",
    description: "Doesn't accurately describe the product or feature.",
  },
  {
    value: "missing-info",
    title: "Couldn't find what I was looking for",
    description: "Missing important information.",
  },
  {
    value: "unclear",
    title: "Hard to understand",
    description: "Too complicated or unclear.",
  },
  {
    value: "bad-examples",
    title: "Code samples errors",
    description: "One or more code samples are incorrect.",
  },
  { value: "other-no", title: "Another reason" },
];

const FEEDBACK_CONTENT_SET_ID = "61a5fb78-4d70-402a-9692-c8ecf3755ed8";

export function FeedbackComponent() {
  const [choice, setChoice] = useState<string>();

  return (
    <div className="feelback-container">
      {!choice ? (
        <Question
          text="Was this page helpful?"
          items={PRESET_YESNO_LIKE_DISLIKE}
          showLabels
          onClick={(choice) => setChoice(choice)}
        />
      ) : (
        <FeelbackTaggedMessage
          contentSetId={FEEDBACK_CONTENT_SET_ID}
          layout="radio-group"
          preset={PRESET_FEEDBACK}
          tags={choice === "y" ? YES_TAGS : NO_TAGS}
          title={choice === "y" ? "What did you like?" : "What can we improve?"}
          placeholder="(optional) Please, provide additional feedback."
          textAnswer={choice}
        />
      )}
    </div>
  );
}
