import React from "react";

export type SwitchProps = {
  checked?: boolean;
  defaultChecked?: boolean;
  onChange?: (checked: boolean) => void;
  disabled?: boolean;
  label?: string;
  description?: string;
  className?: string;
  style?: React.CSSProperties;
  name?: string;
  value?: string;
  size?: "small" | "medium" | "large";
};

export function Switch({
  checked,
  defaultChecked = false,
  onChange,
  disabled = false,
  label,
  description,
  className = "",
  style,
  name,
  value,
  size = "medium",
}: SwitchProps) {
  const [internalChecked, setInternalChecked] = React.useState(defaultChecked);
  const isControlled = typeof checked === "boolean";
  const isChecked = isControlled ? checked : internalChecked;

  const handleChange = (event: React.ChangeEvent<HTMLInputElement>) => {
    const newChecked = event.target.checked;
    if (!isControlled) {
      setInternalChecked(newChecked);
    }
    onChange?.(newChecked);
  };

  const switchId = React.useId();

  const getSizeStyles = () => {
    switch (size) {
      case "small":
        return {
          width: "32px",
          height: "18px",
          thumbSize: "14px",
          thumbOffset: "2px",
        };
      case "large":
        return {
          width: "48px",
          height: "26px",
          thumbSize: "22px",
          thumbOffset: "2px",
        };
      default: // medium
        return {
          width: "40px",
          height: "22px",
          thumbSize: "18px",
          thumbOffset: "2px",
        };
    }
  };

  const sizeStyles = getSizeStyles();

  return (
    <div
      className={className}
      style={{
        display: "flex",
        alignItems: "flex-start",
        gap: "8px",
        ...style,
      }}
    >
      <div
        style={{
          position: "relative",
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
        }}
      >
        <input
          type="checkbox"
          id={switchId}
          name={name}
          value={value}
          checked={isChecked}
          onChange={handleChange}
          disabled={disabled}
          style={{
            position: "absolute",
            opacity: 0,
            cursor: disabled ? "not-allowed" : "pointer",
            margin: 0,
            width: sizeStyles.width,
            height: sizeStyles.height,
          }}
        />
        <div
          style={{
            width: sizeStyles.width,
            height: sizeStyles.height,
            backgroundColor: isChecked
              ? "var(--color-brand-600)"
              : "var(--color-neutral-600)",
            borderRadius: "9999px",
            position: "relative",
            transition: "all 120ms ease",
            cursor: disabled ? "not-allowed" : "pointer",
            opacity: disabled ? 0.5 : 1,
          }}
        >
          <div
            style={{
              width: sizeStyles.thumbSize,
              height: sizeStyles.thumbSize,
              backgroundColor: "#FFFFFF",
              borderRadius: "50%",
              position: "absolute",
              top: sizeStyles.thumbOffset,
              left: isChecked
                ? `calc(100% - ${sizeStyles.thumbSize} - ${sizeStyles.thumbOffset})`
                : sizeStyles.thumbOffset,
              transition: "all 120ms ease",
              boxShadow: "0 2px 4px rgba(0, 0, 0, 0.2)",
            }}
          />
        </div>
      </div>
      {(label || description) && (
        <div style={{ flex: 1, minWidth: 0 }}>
          {label && (
            <label
              htmlFor={switchId}
              style={{
                display: "block",
                color: disabled ? "var(--color-neutral-600)" : "#FFFFFF",
                fontSize: "14px",
                fontWeight: 500,
                cursor: disabled ? "not-allowed" : "pointer",
                fontFamily: "var(--font-body)",
              }}
            >
              {label}
            </label>
          )}
          {description && (
            <div
              style={{
                color: disabled
                  ? "var(--color-neutral-600)"
                  : "var(--color-neutral-400)",
                fontSize: "12px",
                marginTop: "2px",
                fontFamily: "var(--font-body)",
              }}
            >
              {description}
            </div>
          )}
        </div>
      )}
    </div>
  );
}
