import React from "react";

export type RadioProps = {
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

export function Radio({
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
}: RadioProps) {
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

  const radioId = React.useId();

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
          type="radio"
          id={radioId}
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
            borderRadius: "50%",
            backgroundColor: "transparent",
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            transition: "all 120ms ease",
            cursor: disabled ? "not-allowed" : "pointer",
            opacity: disabled ? 0.5 : 1,
          }}
        >
          {isChecked && (
            <div
              style={{
                width: "8px",
                height: "8px",
                borderRadius: "50%",
                backgroundColor: "var(--color-brand-600)",
              }}
            />
          )}
        </div>
      </div>
      {(label || description) && (
        <div style={{ flex: 1, minWidth: 0 }}>
          {label && (
            <label
              htmlFor={radioId}
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

// RadioGroup component for managing multiple radio buttons
export type RadioGroupProps = {
  value?: string;
  defaultValue?: string;
  onChange?: (value: string) => void;
  disabled?: boolean;
  className?: string;
  style?: React.CSSProperties;
  children: React.ReactNode;
  name?: string;
};

export function RadioGroup({
  value,
  defaultValue,
  onChange,
  disabled = false,
  className = "",
  style,
  children,
  name,
}: RadioGroupProps) {
  const [internalValue, setInternalValue] = React.useState(defaultValue || "");
  const isControlled = typeof value === "string";
  const currentValue = isControlled ? value : internalValue;

  const handleChange = (newValue: string) => {
    if (!isControlled) {
      setInternalValue(newValue);
    }
    onChange?.(newValue);
  };

  return (
    <div className={className} style={style}>
      {React.Children.map(children, (child) => {
        if (React.isValidElement<RadioProps>(child) && child.type === Radio) {
          return React.cloneElement(child, {
            name: name,
            checked: child.props.value === currentValue,
            onChange: (checked: boolean) => {
              if (checked && child.props.value) {
                handleChange(child.props.value);
              }
            },
            disabled: disabled || child.props.disabled,
          });
        }
        return child;
      })}
    </div>
  );
}
