import React from "react";
import { tokens } from "@calimero-network/mero-tokens";

export interface SpinnerProps {
  size?: "xs" | "sm" | "md" | "lg" | "xl";
  color?:
    | "primary"
    | "secondary"
    | "muted"
    | "success"
    | "warning"
    | "error"
    | "current";
  thickness?: "thin" | "medium" | "thick";
  className?: string;
  style?: React.CSSProperties;
}

export const Spinner: React.FC<SpinnerProps> = ({
  size = "md",
  color = "primary",
  thickness = "medium",
  className = "",
  style = {},
}) => {
  const sizeMap = {
    xs: 16,
    sm: 20,
    md: 24,
    lg: 32,
    xl: 40,
  };

  const thicknessMap = {
    thin: 2,
    medium: 3,
    thick: 4,
  };

  const colorMap = {
    primary: tokens.color.brand[600].value,
    secondary: tokens.color.neutral[400].value,
    muted: tokens.color.neutral[300].value,
    success: tokens.color.semantic.success.value,
    warning: tokens.color.semantic.warning.value,
    error: tokens.color.semantic.error.value,
    current: "currentColor",
  };

  const spinnerSize = sizeMap[size];
  const strokeWidth = thicknessMap[thickness];
  const radius = (spinnerSize - strokeWidth) / 2;
  const circumference = 2 * Math.PI * radius;

  const spinnerStyle = {
    display: "inline-block",
    width: spinnerSize,
    height: spinnerSize,
    ...style,
  };

  const centerX = spinnerSize / 2;
  const centerY = spinnerSize / 2;

  const circleStyle = {
    fill: "none",
    stroke: colorMap[color],
    strokeWidth,
    strokeLinecap: "round" as const,
    strokeDasharray: circumference,
    strokeDashoffset: circumference * 0.25,
  };

  const groupStyle = {
    animation: "spinner-rotate 1s linear infinite",
    transformOrigin: `${centerX}px ${centerY}px`,
  };

  return (
    <>
      <style>
        {`
          @keyframes spinner-rotate {
            0% {
              transform: rotate(0deg);
            }
            100% {
              transform: rotate(360deg);
            }
          }
        `}
      </style>
      <svg
        className={className}
        style={spinnerStyle}
        viewBox={`0 0 ${spinnerSize} ${spinnerSize}`}
        xmlns="http://www.w3.org/2000/svg"
      >
        <g style={groupStyle}>
          <circle
            cx={centerX}
            cy={centerY}
            r={radius}
            style={circleStyle}
          />
        </g>
      </svg>
    </>
  );
};
