import React, { forwardRef } from "react";
import { tokens } from "@calimero-network/mero-tokens";

export interface TextareaProps {
  value?: string;
  defaultValue?: string;
  placeholder?: string;
  disabled?: boolean;
  readOnly?: boolean;
  required?: boolean;
  rows?: number;
  cols?: number;
  resize?: "none" | "both" | "horizontal" | "vertical";
  size?: "sm" | "md" | "lg";
  variant?: "default" | "filled" | "outlined";
  error?: boolean;
  helperText?: string;
  label?: string;
  className?: string;
  style?: React.CSSProperties;
  onChange?: (event: React.ChangeEvent<HTMLTextAreaElement>) => void;
  onFocus?: (event: React.FocusEvent<HTMLTextAreaElement>) => void;
  onBlur?: (event: React.FocusEvent<HTMLTextAreaElement>) => void;
}

export const Textarea = forwardRef<HTMLTextAreaElement, TextareaProps>(
  (
    {
      value,
      defaultValue,
      placeholder,
      disabled = false,
      readOnly = false,
      required = false,
      rows = 3,
      cols,
      resize = "vertical",
      size = "md",
      variant = "default",
      error = false,
      helperText,
      label,
      className = "",
      style = {},
      onChange,
      onFocus,
      onBlur,
    },
    ref,
  ) => {
    const sizeStyles = {
      sm: {
        padding: "6px 8px",
        fontSize: "14px",
        minHeight: "60px",
      },
      md: {
        padding: "8px 12px",
        fontSize: "16px",
        minHeight: "80px",
      },
      lg: {
        padding: "12px 16px",
        fontSize: "18px",
        minHeight: "100px",
      },
    };

    const variantStyles = {
      default: {
        backgroundColor: "transparent",
        border: `1px solid ${error ? tokens.color.semantic.error.value : tokens.color.neutral[600].value}`,
      },
      filled: {
        backgroundColor: "transparent",
        border: `1px solid ${error ? tokens.color.semantic.error.value : "transparent"}`,
      },
      outlined: {
        backgroundColor: "transparent",
        border: `2px solid ${error ? tokens.color.semantic.error.value : tokens.color.neutral[600].value}`,
      },
    };

    const baseStyle = {
      width: "100%",
      fontFamily: "var(--font-body)", // Use design system font
      color: "#FFFFFF", // White text for better contrast with dark background
      borderRadius: tokens.radius.md.value,
      outline: "none",
      transition: "all 0.2s ease",
      resize,
      fontWeight: "400",
      lineHeight: "1.5",
      ...sizeStyles[size],
      ...variantStyles[variant],
      ...style,
    };

    const focusStyle = {
      borderColor: error
        ? tokens.color.semantic.error.value
        : tokens.color.brand[600].value,
      boxShadow: `0 0 0 3px ${error ? tokens.color.brand[100].value : tokens.color.brand[100].value}`,
    };

    const disabledStyle = {
      backgroundColor: "transparent",
      color: tokens.color.neutral[300].value,
      cursor: "not-allowed",
      opacity: 0.6,
    };

    const labelStyle = {
      display: "block",
      fontSize: "14px",
      fontWeight: 500,
      color: "#FFFFFF", // White text for better contrast
      marginBottom: "4px",
      fontFamily: "var(--font-body)",
      lineHeight: "1.4",
    };

    const helperTextStyle = {
      fontSize: "12px",
      color: error
        ? tokens.color.semantic.error.value
        : tokens.color.neutral[300].value,
      marginTop: "4px",
      fontFamily: "var(--font-body)",
      lineHeight: "1.4",
    };

    const containerStyle = {
      display: "flex",
      flexDirection: "column" as const,
      width: "100%",
    };

    return (
      <div className={className} style={containerStyle}>
        {label && (
          <label style={labelStyle}>
            {label}
            {required && (
              <span
                style={{
                  color: tokens.color.semantic.error.value,
                  marginLeft: "4px",
                }}
              >
                *
              </span>
            )}
          </label>
        )}
        <textarea
          ref={ref}
          value={value}
          defaultValue={defaultValue}
          placeholder={placeholder}
          disabled={disabled}
          readOnly={readOnly}
          required={required}
          rows={rows}
          cols={cols}
          style={{
            ...baseStyle,
            ...(disabled ? disabledStyle : {}),
          }}
          onChange={onChange}
          onFocus={(e) => {
            if (!disabled) {
              Object.assign(e.target.style, focusStyle);
            }
            onFocus?.(e);
          }}
          onBlur={(e) => {
            if (!disabled) {
              Object.assign(e.target.style, {
                borderColor: error
                  ? tokens.color.semantic.error.value
                  : tokens.color.neutral[600].value,
                boxShadow: "none",
              });
            }
            onBlur?.(e);
          }}
        />
        {helperText && <div style={helperTextStyle}>{helperText}</div>}
      </div>
    );
  },
);
