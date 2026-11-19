import React from "react";

export type CheckboxProps = {
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
};

export function Checkbox({
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
}: CheckboxProps) {
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

  const checkboxId = React.useId();

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
          id={checkboxId}
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
            width: "16px",
            height: "16px",
          }}
        />
        <div
          style={{
            width: "16px",
            height: "16px",
            border: `2px solid ${isChecked ? "var(--color-brand-600)" : "var(--color-neutral-600)"}`,
            borderRadius: "4px",
            backgroundColor: isChecked
              ? "var(--color-brand-600)"
              : "transparent",
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            transition: "all 120ms ease",
            cursor: disabled ? "not-allowed" : "pointer",
            opacity: disabled ? 0.5 : 1,
          }}
        >
          {isChecked && (
            <svg
              width="10"
              height="10"
              viewBox="0 0 24 24"
              fill="none"
              stroke="#000000"
              strokeWidth="3"
              style={{ display: "block" }}
            >
              <polyline points="20,6 9,17 4,12" />
            </svg>
          )}
        </div>
      </div>
      {(label || description) && (
        <div style={{ flex: 1, minWidth: 0 }}>
          {label && (
            <label
              htmlFor={checkboxId}
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
