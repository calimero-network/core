import React, { useState, useCallback, useMemo } from "react";
import { tokens } from "@calimero-network/mero-tokens";
import { Button } from "./Button";
import { Text, Heading } from "./Typography";
import { Icon } from "./Icon";
import { Box } from "./Box";
import { Progress } from "./Progress";

export interface WizardStep {
  id: string;
  title: string;
  description?: string;
  icon?: string;
  component: React.ComponentType<WizardStepProps>;
  validation?: (data: any) => boolean | Promise<boolean>;
  isOptional?: boolean;
  isDisabled?: boolean;
}

export interface WizardStepProps {
  data: any;
  updateData: (data: any) => void;
  goToStep: (stepId: string) => void;
  isFirstStep: boolean;
  isLastStep: boolean;
  stepIndex: number;
  totalSteps: number;
}

export interface BuildWizardProps {
  steps: WizardStep[];
  initialData?: any;
  onComplete: (data: any) => void;
  onCancel?: () => void;
  onStepChange?: (stepId: string, stepIndex: number) => void;
  showProgress?: boolean;
  showStepNumbers?: boolean;
  allowSkip?: boolean;
  className?: string;
  style?: React.CSSProperties;
}

export function BuildWizard({
  steps,
  initialData = {},
  onComplete,
  onCancel,
  onStepChange,
  showProgress = true,
  showStepNumbers = true,
  allowSkip = false,
  className = "",
  style = {},
}: BuildWizardProps) {
  const [currentStepIndex, setCurrentStepIndex] = useState(0);
  const [data, setData] = useState(initialData);
  const [isValidating, setIsValidating] = useState(false);
  const [validationErrors, setValidationErrors] = useState<
    Record<string, string>
  >({});

  const currentStep = steps[currentStepIndex];
  const isFirstStep = currentStepIndex === 0;
  const isLastStep = currentStepIndex === steps.length - 1;
  const progress = ((currentStepIndex + 1) / steps.length) * 100;

  const updateData = useCallback((newData: any) => {
    setData((prev: any) => ({ ...prev, ...newData }));
  }, []);

  const goToStep = useCallback(
    (stepId: string) => {
      const stepIndex = steps.findIndex((step) => step.id === stepId);
      if (stepIndex !== -1) {
        setCurrentStepIndex(stepIndex);
        onStepChange?.(stepId, stepIndex);
      }
    },
    [steps, onStepChange],
  );

  const validateCurrentStep = useCallback(async () => {
    if (!currentStep.validation) return true;

    setIsValidating(true);
    setValidationErrors({});

    try {
      const isValid = await currentStep.validation(data);
      if (!isValid) {
        setValidationErrors({
          [currentStep.id]: "Please complete all required fields",
        });
      }
      return isValid;
    } catch (error) {
      setValidationErrors({ [currentStep.id]: "Validation failed" });
      return false;
    } finally {
      setIsValidating(false);
    }
  }, [currentStep, data]);

  const handleNext = useCallback(async () => {
    const isValid = await validateCurrentStep();
    if (isValid) {
      if (isLastStep) {
        onComplete(data);
      } else {
        setCurrentStepIndex((prev) => prev + 1);
        onStepChange?.(steps[currentStepIndex + 1].id, currentStepIndex + 1);
      }
    }
  }, [
    validateCurrentStep,
    isLastStep,
    onComplete,
    data,
    steps,
    currentStepIndex,
    onStepChange,
  ]);

  const handlePrevious = useCallback(() => {
    if (!isFirstStep) {
      setCurrentStepIndex((prev) => prev - 1);
      onStepChange?.(steps[currentStepIndex - 1].id, currentStepIndex - 1);
    }
  }, [isFirstStep, steps, currentStepIndex, onStepChange]);

  const handleSkip = useCallback(() => {
    if (allowSkip && !isLastStep) {
      setCurrentStepIndex((prev) => prev + 1);
      onStepChange?.(steps[currentStepIndex + 1].id, currentStepIndex + 1);
    }
  }, [allowSkip, isLastStep, steps, currentStepIndex, onStepChange]);

  const stepProps: WizardStepProps = {
    data,
    updateData,
    goToStep,
    isFirstStep,
    isLastStep,
    stepIndex: currentStepIndex,
    totalSteps: steps.length,
  };

  const StepComponent = currentStep.component;

  return (
    <Box
      className={className}
      style={{
        display: "flex",
        flexDirection: "column",
        height: "100%",
        background: "var(--color-background-primary)",
        borderRadius: "12px",
        border: `1px solid ${tokens.color.neutral[700].value}`,
        overflow: "hidden",
        ...style,
      }}
    >
      {/* Header */}
      <Box
        style={{
          padding: "24px",
          borderBottom: `1px solid ${tokens.color.neutral[700].value}`,
          background: "var(--color-background-secondary)",
        }}
      >
        <Box
          style={{
            display: "flex",
            alignItems: "center",
            gap: "16px",
            marginBottom: "16px",
          }}
        >
          {currentStep.icon && (
            <Box
              style={{
                width: "40px",
                height: "40px",
                borderRadius: "8px",
                background: "var(--color-brand-600)",
                display: "flex",
                alignItems: "center",
                justifyContent: "center",
                color: "white",
              }}
            >
              <Icon name={currentStep.icon} size="md" />
            </Box>
          )}
          <Box style={{ flex: 1 }}>
            <Heading
              size="lg"
              style={{ margin: 0, color: "var(--color-neutral-100)" }}
            >
              {currentStep.title}
            </Heading>
            {currentStep.description && (
              <Text size="md" color="muted" style={{ margin: "4px 0 0 0" }}>
                {currentStep.description}
              </Text>
            )}
          </Box>
          {showStepNumbers && (
            <Box
              style={{
                padding: "8px 12px",
                background: "var(--color-neutral-700)",
                borderRadius: "6px",
                color: "var(--color-neutral-200)",
                fontSize: "14px",
                fontWeight: "600",
              }}
            >
              {currentStepIndex + 1} of {steps.length}
            </Box>
          )}
        </Box>

        {showProgress && (
          <Box>
            <Box
              style={{
                display: "flex",
                justifyContent: "space-between",
                marginBottom: "8px",
              }}
            >
              <Text size="sm" color="muted">
                Progress
              </Text>
              <Text size="sm" color="muted">
                {Math.round(progress)}%
              </Text>
            </Box>
            <Progress value={progress} size="sm" />
          </Box>
        )}
      </Box>

      {/* Step Navigation */}
      <Box
        style={{
          padding: "16px 24px",
          background: "var(--color-background-secondary)",
          borderBottom: `1px solid ${tokens.color.neutral[700].value}`,
        }}
      >
        <Box style={{ display: "flex", gap: "8px", overflowX: "auto" }}>
          {steps.map((step, index) => (
            <Box
              key={step.id}
              onClick={() => !step.isDisabled && goToStep(step.id)}
              style={{
                display: "flex",
                alignItems: "center",
                gap: "8px",
                padding: "8px 12px",
                borderRadius: "6px",
                background:
                  index === currentStepIndex
                    ? "var(--color-brand-600)"
                    : index < currentStepIndex
                      ? "var(--color-neutral-600)"
                      : "transparent",
                color:
                  index === currentStepIndex
                    ? "white"
                    : index < currentStepIndex
                      ? "var(--color-neutral-200)"
                      : "var(--color-neutral-400)",
                cursor: step.isDisabled ? "not-allowed" : "pointer",
                opacity: step.isDisabled ? 0.5 : 1,
                transition: "all 0.2s ease",
                whiteSpace: "nowrap",
                fontSize: "14px",
                fontWeight: "500",
              }}
            >
              {step.icon && <Icon name={step.icon} size="sm" />}
              <Text size="sm" style={{ margin: 0 }}>
                {step.title}
              </Text>
              {step.isOptional && (
                <Text size="xs" style={{ margin: 0, opacity: 0.7 }}>
                  (Optional)
                </Text>
              )}
            </Box>
          ))}
        </Box>
      </Box>

      {/* Content */}
      <Box style={{ flex: 1, padding: "24px", overflow: "auto" }}>
        {validationErrors[currentStep.id] && (
          <Box
            style={{
              padding: "12px 16px",
              background: "var(--color-semantic-error)20",
              border: `1px solid ${tokens.color.semantic.error.value}40`,
              borderRadius: "8px",
              marginBottom: "16px",
              color: "var(--color-semantic-error)",
              fontSize: "14px",
            }}
          >
            {validationErrors[currentStep.id]}
          </Box>
        )}

        <StepComponent {...stepProps} />
      </Box>

      {/* Footer */}
      <Box
        style={{
          padding: "24px",
          borderTop: `1px solid ${tokens.color.neutral[700].value}`,
          background: "var(--color-background-secondary)",
          display: "flex",
          justifyContent: "space-between",
          alignItems: "center",
        }}
      >
        <Box style={{ display: "flex", gap: "12px" }}>
          {onCancel && (
            <Button variant="secondary" onClick={onCancel}>
              Cancel
            </Button>
          )}
          {!isFirstStep && (
            <Button variant="secondary" onClick={handlePrevious}>
              <Icon name="arrow-left" size="sm" />
              Previous
            </Button>
          )}
        </Box>

        <Box style={{ display: "flex", gap: "12px" }}>
          {allowSkip && !isLastStep && !currentStep.isOptional && (
            <Button variant="secondary" onClick={handleSkip}>
              Skip
            </Button>
          )}
          <Button
            variant="primary"
            onClick={handleNext}
            disabled={isValidating || currentStep.isDisabled}
          >
            {isValidating ? (
              <>
                <Icon name="loader" size="sm" />
                Validating...
              </>
            ) : isLastStep ? (
              "Complete"
            ) : (
              <>
                Next
                <Icon name="arrow-right" size="sm" />
              </>
            )}
          </Button>
        </Box>
      </Box>
    </Box>
  );
}

// Predefined step components for common use cases
export interface TextInputStepProps extends WizardStepProps {
  label: string;
  placeholder?: string;
  required?: boolean;
  multiline?: boolean;
  rows?: number;
}

export function TextInputStep({
  data,
  updateData,
  label,
  placeholder,
  required = false,
  multiline = false,
  rows = 3,
}: TextInputStepProps) {
  const value = data[label.toLowerCase().replace(/\s+/g, "_")] || "";

  return (
    <Box>
      <Text
        size="md"
        weight="medium"
        style={{ marginBottom: "8px", color: "var(--color-neutral-200)" }}
      >
        {label}{" "}
        {required && (
          <span style={{ color: "var(--color-semantic-error)" }}>*</span>
        )}
      </Text>
      {multiline ? (
        <textarea
          value={value}
          onChange={(e) =>
            updateData({
              [label.toLowerCase().replace(/\s+/g, "_")]: e.target.value,
            })
          }
          placeholder={placeholder}
          rows={rows}
          required={required}
          style={{
            width: "100%",
            padding: "12px",
            borderRadius: "8px",
            border: `1px solid ${tokens.color.neutral[600].value}`,
            background: "var(--color-background-primary)",
            color: "var(--color-neutral-100)",
            fontSize: "14px",
            fontFamily: "var(--font-body)",
            resize: "vertical",
            minHeight: "100px",
          }}
        />
      ) : (
        <input
          type="text"
          value={value}
          onChange={(e) =>
            updateData({
              [label.toLowerCase().replace(/\s+/g, "_")]: e.target.value,
            })
          }
          placeholder={placeholder}
          required={required}
          style={{
            width: "100%",
            padding: "12px",
            borderRadius: "8px",
            border: `1px solid ${tokens.color.neutral[600].value}`,
            background: "var(--color-background-primary)",
            color: "var(--color-neutral-100)",
            fontSize: "14px",
            fontFamily: "var(--font-body)",
          }}
        />
      )}
    </Box>
  );
}

export interface SelectStepProps extends WizardStepProps {
  label: string;
  options: Array<{ value: string; label: string; disabled?: boolean }>;
  required?: boolean;
  multiple?: boolean;
}

export function SelectStep({
  data,
  updateData,
  label,
  options,
  required = false,
  multiple = false,
}: SelectStepProps) {
  const fieldName = label.toLowerCase().replace(/\s+/g, "_");
  const value = data[fieldName] || (multiple ? [] : "");

  return (
    <Box>
      <Text
        size="md"
        weight="medium"
        style={{ marginBottom: "8px", color: "var(--color-neutral-200)" }}
      >
        {label}{" "}
        {required && (
          <span style={{ color: "var(--color-semantic-error)" }}>*</span>
        )}
      </Text>
      <select
        value={value}
        onChange={(e) => {
          const newValue = multiple
            ? Array.from(e.target.selectedOptions, (option) => option.value)
            : e.target.value;
          updateData({ [fieldName]: newValue });
        }}
        required={required}
        multiple={multiple}
        style={{
          width: "100%",
          padding: "12px",
          borderRadius: "8px",
          border: `1px solid ${tokens.color.neutral[600].value}`,
          background: "var(--color-background-primary)",
          color: "var(--color-neutral-100)",
          fontSize: "14px",
          fontFamily: "var(--font-body)",
        }}
      >
        {options.map((option) => (
          <option
            key={option.value}
            value={option.value}
            disabled={option.disabled}
            style={{
              background: "var(--color-background-primary)",
              color: "var(--color-neutral-100)",
            }}
          >
            {option.label}
          </option>
        ))}
      </select>
    </Box>
  );
}

export interface CheckboxStepProps extends WizardStepProps {
  label: string;
  options: Array<{ value: string; label: string; description?: string }>;
  required?: boolean;
}

export function CheckboxStep({
  data,
  updateData,
  label,
  options,
  required = false,
}: CheckboxStepProps) {
  const fieldName = label.toLowerCase().replace(/\s+/g, "_");
  const value = data[fieldName] || [];

  const handleChange = (optionValue: string, checked: boolean) => {
    const newValue = checked
      ? [...value, optionValue]
      : value.filter((v: string) => v !== optionValue);
    updateData({ [fieldName]: newValue });
  };

  return (
    <Box>
      <Text
        size="md"
        weight="medium"
        style={{ marginBottom: "16px", color: "var(--color-neutral-200)" }}
      >
        {label}{" "}
        {required && (
          <span style={{ color: "var(--color-semantic-error)" }}>*</span>
        )}
      </Text>
      <Box style={{ display: "flex", flexDirection: "column", gap: "12px" }}>
        {options.map((option) => (
          <Box
            key={option.value}
            style={{
              display: "flex",
              alignItems: "flex-start",
              gap: "12px",
              padding: "12px",
              borderRadius: "8px",
              border: `1px solid ${tokens.color.neutral[700].value}`,
              background: "var(--color-background-secondary)",
            }}
          >
            <input
              type="checkbox"
              checked={value.includes(option.value)}
              onChange={(e) => handleChange(option.value, e.target.checked)}
              style={{
                margin: "2px 0 0 0",
                accentColor: "var(--color-brand-600)",
              }}
            />
            <Box style={{ flex: 1 }}>
              <Text
                size="md"
                weight="medium"
                style={{ margin: 0, color: "var(--color-neutral-100)" }}
              >
                {option.label}
              </Text>
              {option.description && (
                <Text size="sm" color="muted" style={{ margin: "4px 0 0 0" }}>
                  {option.description}
                </Text>
              )}
            </Box>
          </Box>
        ))}
      </Box>
    </Box>
  );
}
