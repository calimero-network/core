import React from "react";

type InputProps = {
  value?: string;
  defaultValue?: string;
  onChange?: (event: React.ChangeEvent<HTMLInputElement>) => void;
  placeholder?: string;
  disabled?: boolean;
  className?: string;
  style?: React.CSSProperties;
  type?: "text" | "password" | "email" | "number" | "search";
  name?: string;
  id?: string;
  autoComplete?: string;
  autoFocus?: boolean;
  maxLength?: number;
  min?: number | string;
  max?: number | string;
  step?: number | string;
};

export function Input({
  value,
  defaultValue,
  onChange,
  placeholder,
  disabled = false,
  className = "",
  style,
  type = "text",
  name,
  id,
  autoComplete,
  autoFocus,
  maxLength,
  min,
  max,
  step,
}: InputProps) {
  const baseStyles: React.CSSProperties = {
    height: "40px",
    width: "100%",
    padding: "0 12px",
    borderRadius: "12px",
    border: "1px solid #404040",
    backgroundColor: "transparent",
    color: "#FFFFFF", // Ensure white text for better contrast
    outline: "none",
    fontFamily: "var(--font-body)", // Use design system font
    fontSize: "14px",
    fontWeight: "400",
    lineHeight: "1.5",
    transition: "border-color 120ms ease, box-shadow 120ms ease",
  };

  const [isFocused, setIsFocused] = React.useState(false);
  const [isHover, setIsHover] = React.useState(false);

  let dynamicBorder = "1px solid #404040";
  let dynamicShadow: string | undefined = undefined;
  if (!disabled) {
    if (isFocused) {
      dynamicBorder = "1px solid #6B7280";
      dynamicShadow = "0 0 0 3px rgba(107, 114, 128, 0.25)";
    } else if (isHover) {
      dynamicBorder = "1px solid #505050";
    } else {
      dynamicBorder = "1px solid #404040";
    }
  }

  return (
    <input
      id={id}
      name={name}
      type={type}
      value={value}
      defaultValue={defaultValue}
      onChange={onChange}
      placeholder={placeholder}
      disabled={disabled}
      autoComplete={autoComplete}
      autoFocus={autoFocus}
      maxLength={maxLength}
      min={min as any}
      max={max as any}
      step={step as any}
      onFocus={() => setIsFocused(true)}
      onBlur={() => setIsFocused(false)}
      onMouseEnter={() => setIsHover(true)}
      onMouseLeave={() => setIsHover(false)}
      className={className}
      style={{
        ...baseStyles,
        border: dynamicBorder,
        boxShadow: dynamicShadow,
        cursor: disabled ? "not-allowed" : "text",
        ...style,
      }}
    />
  );
}
