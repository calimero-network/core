import React from "react";
import { tokens } from "@calimero-network/mero-tokens";

export interface SearchHighlightProps {
  text: string;
  searchTerm: string;
  highlightColor?: string;
  caseSensitive?: boolean;
  className?: string;
  style?: React.CSSProperties;
}

export function SearchHighlight({
  text,
  searchTerm,
  highlightColor = tokens.color.brand[600].value,
  caseSensitive = false,
  className = "",
  style = {},
}: SearchHighlightProps) {
  if (!searchTerm.trim()) {
    return (
      <span className={className} style={style}>
        {text}
      </span>
    );
  }

  const flags = caseSensitive ? "g" : "gi";
  const regex = new RegExp(
    `(${searchTerm.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")})`,
    flags,
  );
  const parts = text.split(regex);

  return (
    <span className={className} style={style}>
      {parts.map((part, index) => {
        const isMatch = regex.test(part);
        return (
          <span
            key={index}
            style={
              isMatch
                ? {
                    backgroundColor: highlightColor + "40",
                    color: highlightColor,
                    fontWeight: 600,
                    padding: "1px 2px",
                    borderRadius: "2px",
                  }
                : {}
            }
          >
            {part}
          </span>
        );
      })}
    </span>
  );
}
