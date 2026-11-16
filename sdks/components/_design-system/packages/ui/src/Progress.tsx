import React from "react";
import { tokens } from "@calimero-network/mero-tokens";

export interface ProgressProps {
  value: number;
  max?: number;
  size?: "sm" | "md" | "lg";
  variant?: "default" | "success" | "warning" | "error";
  showValue?: boolean;
  label?: string;
  className?: string;
  style?: React.CSSProperties;
}

export const Progress: React.FC<ProgressProps> = ({
  value,
  max = 100,
  size = "md",
  variant = "default",
  showValue = false,
  label,
  className = "",
  style = {},
}) => {
  const percentage = Math.min(Math.max((value / max) * 100, 0), 100);

  const sizeStyles = {
    sm: {
      height: "4px",
      fontSize: "12px",
    },
    md: {
      height: "8px",
      fontSize: "14px",
    },
    lg: {
      height: "12px",
      fontSize: "16px",
    },
  };

  const variantStyles = {
    default: {
      backgroundColor: tokens.color.brand[600].value,
    },
    success: {
      backgroundColor: tokens.color.semantic.success.value,
    },
    warning: {
      backgroundColor: tokens.color.semantic.warning.value,
    },
    error: {
      backgroundColor: tokens.color.semantic.error.value,
    },
  };

  const containerStyle = {
    width: "100%",
    backgroundColor: tokens.color.background.tertiary.value,
    borderRadius: "9999px",
    overflow: "hidden",
    ...sizeStyles[size],
    ...style,
  };

  const progressStyle = {
    height: "100%",
    width: `${percentage}%`,
    backgroundColor: variantStyles[variant].backgroundColor,
    borderRadius: "9999px",
    transition: "width 0.3s ease",
  };

  const labelStyle = {
    fontSize: "14px",
    fontWeight: 500,
    color: "#FFFFFF", // White text for better contrast with dark background
    marginBottom: "4px",
  };

  const valueStyle = {
    fontSize: sizeStyles[size].fontSize,
    fontWeight: 500,
    color: "#FFFFFF", // White text for better contrast with dark background
    marginTop: "4px",
  };

  return (
    <div className={className}>
      {label && <div style={labelStyle}>{label}</div>}
      <div style={containerStyle}>
        <div style={progressStyle} />
      </div>
      {showValue && <div style={valueStyle}>{Math.round(percentage)}%</div>}
    </div>
  );
};

export interface CircularProgressProps {
  value: number;
  max?: number;
  size?: "sm" | "md" | "lg" | "xl";
  variant?: "default" | "success" | "warning" | "error";
  showValue?: boolean;
  label?: string;
  className?: string;
  style?: React.CSSProperties;
}

export const CircularProgress: React.FC<CircularProgressProps> = ({
  value,
  max = 100,
  size = "md",
  variant = "default",
  showValue = false,
  label,
  className = "",
  style = {},
}) => {
  const percentage = Math.min(Math.max((value / max) * 100, 0), 100);
  const circumference = 2 * Math.PI * 45; // radius = 45
  const strokeDashoffset = circumference - (percentage / 100) * circumference;

  const sizeMap = {
    sm: 60,
    md: 80,
    lg: 100,
    xl: 120,
  };

  const sizeValue = sizeMap[size];
  const radius = sizeValue / 2 - 8;

  const variantStyles = {
    default: tokens.color.brand[600].value,
    success: tokens.color.semantic.success.value,
    warning: tokens.color.semantic.warning.value,
    error: tokens.color.semantic.error.value,
  };

  const containerStyle = {
    display: "inline-flex",
    flexDirection: "column" as const,
    alignItems: "center",
    backgroundColor: "var(--color-neutral-900)", // Ensure proper background color
    ...style,
  };

  const svgStyle = {
    transform: "rotate(-90deg)",
  };

  const labelStyle = {
    fontSize: "14px",
    fontWeight: 500,
    color: "#FFFFFF", // White text for better contrast with dark background
    marginBottom: "8px",
  };

  const valueStyle = {
    position: "absolute" as const,
    top: "50%",
    left: "50%",
    transform: "translate(-50%, -50%)",
    fontSize: "14px",
    fontWeight: 600,
    color: "#FFFFFF", // White text for better contrast with dark background
  };

  return (
    <div className={className} style={containerStyle}>
      {label && <div style={labelStyle}>{label}</div>}
      <div
        style={{
          position: "relative",
          display: "inline-block",
          backgroundColor: "var(--color-neutral-900)", // Ensure proper background color
          borderRadius: "50%", // Make it circular to match the progress circle
          width: sizeValue,
          height: sizeValue,
          minWidth: sizeValue,
          minHeight: sizeValue,
        }}
      >
        <svg
          width={sizeValue}
          height={sizeValue}
          style={{
            ...svgStyle,
            backgroundColor: "var(--color-neutral-900)", // Ensure SVG has proper background
          }}
        >
          {/* Background circle to ensure proper background color */}
          <circle
            cx={sizeValue / 2}
            cy={sizeValue / 2}
            r={sizeValue / 2}
            fill="var(--color-neutral-900)"
          />
          <circle
            cx={sizeValue / 2}
            cy={sizeValue / 2}
            r={radius}
            stroke={tokens.color.background.tertiary.value}
            strokeWidth="8"
            fill="none"
          />
          <circle
            cx={sizeValue / 2}
            cy={sizeValue / 2}
            r={radius}
            stroke={variantStyles[variant]}
            strokeWidth="8"
            fill="none"
            strokeDasharray={circumference}
            strokeDashoffset={strokeDashoffset}
            strokeLinecap="round"
            style={{
              transition: "stroke-dashoffset 0.3s ease",
            }}
          />
        </svg>
        {showValue && <div style={valueStyle}>{Math.round(percentage)}%</div>}
      </div>
    </div>
  );
};
