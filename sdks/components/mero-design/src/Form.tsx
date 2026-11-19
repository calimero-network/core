import React, { createContext, useContext, useCallback } from "react";
import { tokens } from "@calimero-network/mero-tokens";

interface FormContextValue {
  errors: Record<string, string>;
  touched: Record<string, boolean>;
  values: Record<string, any>;
  setFieldValue: (name: string, value: any) => void;
  setFieldTouched: (name: string, touched: boolean) => void;
  setFieldError: (name: string, error: string) => void;
  validateField: (name: string) => Promise<string | undefined>;
}

const FormContext = createContext<FormContextValue | null>(null);

export const useFormContext = () => {
  const context = useContext(FormContext);
  if (!context) {
    throw new Error("useFormContext must be used within a Form component");
  }
  return context;
};

interface FormProps extends React.FormHTMLAttributes<HTMLFormElement> {
  initialValues?: Record<string, any>;
  onSubmit?: (values: Record<string, any>) => void | Promise<void>;
  validationSchema?: Record<string, (value: any) => string | undefined>;
  children: React.ReactNode;
  className?: string;
  style?: React.CSSProperties;
}

export const Form: React.FC<FormProps> = ({
  initialValues = {},
  onSubmit,
  validationSchema = {},
  children,
  className = "",
  style = {},
  ...props
}) => {
  const [values, setValues] = React.useState(initialValues);
  const [errors, setErrors] = React.useState<Record<string, string>>({});
  const [touched, setTouched] = React.useState<Record<string, boolean>>({});

  const setFieldValue = useCallback(
    (name: string, value: any) => {
      setValues((prev) => ({ ...prev, [name]: value }));
      // Clear error when user starts typing
      if (errors[name]) {
        setErrors((prev) => ({ ...prev, [name]: "" }));
      }
    },
    [errors],
  );

  const setFieldTouched = useCallback((name: string, isTouched: boolean) => {
    setTouched((prev) => ({ ...prev, [name]: isTouched }));
  }, []);

  const setFieldError = useCallback((name: string, error: string) => {
    setErrors((prev) => ({ ...prev, [name]: error }));
  }, []);

  const validateField = useCallback(
    async (name: string): Promise<string | undefined> => {
      const validator = validationSchema[name];
      if (!validator) return undefined;

      const error = validator(values[name]);
      if (error) {
        setFieldError(name, error);
        return error;
      }
      return undefined;
    },
    [validationSchema, values, setFieldError],
  );

  const handleSubmit = async (e: React.FormEvent<HTMLFormElement>) => {
    e.preventDefault();

    // Validate all fields
    const validationErrors: Record<string, string> = {};
    for (const [fieldName, validator] of Object.entries(validationSchema)) {
      const error = validator(values[fieldName]);
      if (error) {
        validationErrors[fieldName] = error;
      }
    }

    if (Object.keys(validationErrors).length > 0) {
      setErrors(validationErrors);
      return;
    }

    if (onSubmit) {
      await onSubmit(values);
    }
  };

  const contextValue: FormContextValue = {
    errors,
    touched,
    values,
    setFieldValue,
    setFieldTouched,
    setFieldError,
    validateField,
  };

  const formStyle: React.CSSProperties = {
    display: "flex",
    flexDirection: "column",
    gap: tokens.space["4"].value,
    ...style,
  };

  return (
    <FormContext.Provider value={contextValue}>
      <form
        className={className}
        style={formStyle}
        onSubmit={handleSubmit}
        {...props}
      >
        {children}
      </form>
    </FormContext.Provider>
  );
};

interface FieldsetProps
  extends React.FieldsetHTMLAttributes<HTMLFieldSetElement> {
  legend?: string;
  children: React.ReactNode;
  className?: string;
  style?: React.CSSProperties;
}

export const Fieldset: React.FC<FieldsetProps> = ({
  legend,
  children,
  className = "",
  style = {},
  ...props
}) => {
  const fieldsetStyle: React.CSSProperties = {
    border: `1px solid ${tokens.color.neutral[600].value}`,
    borderRadius: tokens.radius.md.value,
    padding: tokens.space["4"].value,
    margin: 0,
    ...style,
  };

  const legendStyle: React.CSSProperties = {
    padding: `0 ${tokens.space["2"].value}`,
    fontSize: "14px",
    fontWeight: 500,
    color: "#FFFFFF", // White text for better contrast
    fontFamily: "var(--font-body)",
  };

  return (
    <fieldset className={className} style={fieldsetStyle} {...props}>
      {legend && <legend style={legendStyle}>{legend}</legend>}
      {children}
    </fieldset>
  );
};

interface FormFieldProps {
  name: string;
  label?: string;
  required?: boolean;
  children: React.ReactNode;
  className?: string;
  style?: React.CSSProperties;
}

export const FormField: React.FC<FormFieldProps> = ({
  name,
  label,
  required = false,
  children,
  className = "",
  style = {},
}) => {
  const { errors, touched } = useFormContext();
  const hasError = touched[name] && errors[name];

  const fieldStyle: React.CSSProperties = {
    display: "flex",
    flexDirection: "column",
    gap: tokens.space["2"].value,
    ...style,
  };

  const labelStyle: React.CSSProperties = {
    fontSize: "14px",
    fontWeight: 500,
    color: hasError ? tokens.color.semantic.error.value : "#FFFFFF", // White text for better contrast
    fontFamily: "var(--font-body)",
  };

  const errorStyle: React.CSSProperties = {
    fontSize: "12px",
    color: tokens.color.semantic.error.value,
    marginTop: "4px", // Better spacing
    fontFamily: "var(--font-body)",
  };

  return (
    <div className={className} style={fieldStyle}>
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
      {children}
      {hasError && <span style={errorStyle}>{errors[name]}</span>}
    </div>
  );
};

interface FormGroupProps extends React.HTMLAttributes<HTMLDivElement> {
  children: React.ReactNode;
  direction?: "row" | "column";
  gap?: "sm" | "md" | "lg";
  className?: string;
  style?: React.CSSProperties;
}

export const FormGroup: React.FC<FormGroupProps> = ({
  children,
  direction = "column",
  gap = "md",
  className = "",
  style = {},
  ...props
}) => {
  const gapMap = {
    sm: tokens.space["2"].value,
    md: tokens.space["4"].value,
    lg: tokens.space["6"].value,
  };

  const groupStyle: React.CSSProperties = {
    display: "flex",
    flexDirection: direction,
    gap: gapMap[gap],
    ...style,
  };

  return (
    <div className={className} style={groupStyle} {...props}>
      {children}
    </div>
  );
};
