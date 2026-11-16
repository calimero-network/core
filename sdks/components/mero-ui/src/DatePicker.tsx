import React, { useState, useRef, useEffect } from "react";
import { tokens } from "@calimero-network/mero-tokens";
import { Input } from "./Input";
import { Button } from "./Button";
import { Icon } from "./Icon";

export interface DatePickerProps {
  value?: Date | null;
  onChange?: (date: Date | null) => void;
  placeholder?: string;
  disabled?: boolean;
  format?: string;
  minDate?: Date;
  maxDate?: Date;
  showTime?: boolean;
  timeFormat?: "12h" | "24h";
  className?: string;
  style?: React.CSSProperties;
  error?: boolean;
  errorMessage?: string;
  label?: string;
  required?: boolean;
  size?: "small" | "medium" | "large";
}

export function DatePicker({
  value,
  onChange,
  placeholder = "Select date",
  disabled = false,
  format = "MM/dd/yyyy",
  minDate,
  maxDate,
  showTime = false,
  timeFormat = "12h",
  className = "",
  style = {},
  error = false,
  errorMessage,
  label,
  required = false,
  size = "medium",
}: DatePickerProps) {
  const [isOpen, setIsOpen] = useState(false);
  const [inputValue, setInputValue] = useState("");
  const [currentMonth, setCurrentMonth] = useState(new Date());
  const [selectedTime, setSelectedTime] = useState({
    hours: 0,
    minutes: 0,
    ampm: "AM",
  });
  const containerRef = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  // Format date for display
  const formatDate = (date: Date): string => {
    const options: Intl.DateTimeFormatOptions = {
      year: "numeric",
      month: "2-digit",
      day: "2-digit",
    };

    if (showTime) {
      options.hour = timeFormat === "12h" ? "numeric" : "2-digit";
      options.minute = "2-digit";
      if (timeFormat === "12h") {
        options.hour12 = true;
      }
    }

    return date.toLocaleDateString("en-US", options);
  };

  // Parse date from input
  const parseDate = (dateString: string): Date | null => {
    if (!dateString) return null;

    const date = new Date(dateString);
    return isNaN(date.getTime()) ? null : date;
  };

  // Initialize input value
  useEffect(() => {
    if (value) {
      setInputValue(formatDate(value));
      if (showTime) {
        const hours = value.getHours();
        const minutes = value.getMinutes();
        setSelectedTime({
          hours: timeFormat === "12h" ? hours % 12 || 12 : hours,
          minutes,
          ampm: hours >= 12 ? "PM" : "AM",
        });
      }
    } else {
      setInputValue("");
    }
  }, [value, showTime, timeFormat]);

  // Close calendar when clicking outside
  useEffect(() => {
    const handleClickOutside = (event: MouseEvent) => {
      if (
        containerRef.current &&
        !containerRef.current.contains(event.target as Node)
      ) {
        setIsOpen(false);
      }
    };

    document.addEventListener("mousedown", handleClickOutside);
    return () => document.removeEventListener("mousedown", handleClickOutside);
  }, []);

  // Handle input change
  const handleInputChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    const newValue = e.target.value;
    setInputValue(newValue);

    const parsedDate = parseDate(newValue);
    if (parsedDate) {
      onChange?.(parsedDate);
    }
  };

  // Handle date selection
  const handleDateSelect = (date: Date) => {
    let finalDate = new Date(date);

    if (showTime) {
      let hours = selectedTime.hours;
      if (timeFormat === "12h" && selectedTime.ampm === "PM" && hours !== 12) {
        hours += 12;
      } else if (
        timeFormat === "12h" &&
        selectedTime.ampm === "AM" &&
        hours === 12
      ) {
        hours = 0;
      }

      finalDate.setHours(hours, selectedTime.minutes, 0, 0);
    }

    onChange?.(finalDate);
    setInputValue(formatDate(finalDate));
    setIsOpen(false);
  };

  // Handle time change
  const handleTimeChange = (
    field: "hours" | "minutes" | "ampm",
    value: string | number,
  ) => {
    setSelectedTime((prev) => ({
      ...prev,
      [field]: value,
    }));
  };

  // Generate calendar days
  const generateCalendarDays = () => {
    const year = currentMonth.getFullYear();
    const month = currentMonth.getMonth();
    const firstDay = new Date(year, month, 1);
    const lastDay = new Date(year, month + 1, 0);
    const startDate = new Date(firstDay);
    startDate.setDate(startDate.getDate() - firstDay.getDay());

    const days = [];
    const currentDate = new Date(startDate);

    for (let i = 0; i < 42; i++) {
      const isCurrentMonth = currentDate.getMonth() === month;
      const isToday = currentDate.toDateString() === new Date().toDateString();
      const isSelected =
        value && currentDate.toDateString() === value.toDateString();
      const isDisabled =
        (minDate && currentDate < minDate) ||
        (maxDate && currentDate > maxDate);

      days.push({
        date: new Date(currentDate),
        isCurrentMonth,
        isToday,
        isSelected,
        isDisabled,
      });

      currentDate.setDate(currentDate.getDate() + 1);
    }

    return days;
  };

  // Navigate months
  const navigateMonth = (direction: "prev" | "next") => {
    setCurrentMonth((prev) => {
      const newMonth = new Date(prev);
      newMonth.setMonth(prev.getMonth() + (direction === "next" ? 1 : -1));
      return newMonth;
    });
  };

  const sizeStyles = {
    small: { height: "32px", fontSize: "12px" },
    medium: { height: "40px", fontSize: "14px" },
    large: { height: "48px", fontSize: "16px" },
  };

  const currentSizeStyle = sizeStyles[size];

  return (
    <div
      ref={containerRef}
      className={className}
      style={{ position: "relative", ...style }}
    >
      {label && (
        <label
          style={{
            display: "block",
            fontSize: "14px",
            fontWeight: 500,
            color: "#FFFFFF",
            marginBottom: "8px",
            fontFamily: "var(--font-body)",
          }}
        >
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

      <div style={{ position: "relative" }}>
        <div onClick={() => !disabled && setIsOpen(!isOpen)}>
          <Input
            value={inputValue}
            onChange={handleInputChange}
            placeholder={placeholder}
            disabled={disabled}
            style={{
              ...currentSizeStyle,
              paddingRight: "40px",
              cursor: "pointer",
            }}
          />
        </div>

        <button
          type="button"
          style={{
            position: "absolute",
            right: "8px",
            top: "50%",
            transform: "translateY(-50%)",
            background: "none",
            border: "none",
            color: tokens.color.neutral[400].value,
            cursor: disabled ? "not-allowed" : "pointer",
            padding: "4px",
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
          }}
          onClick={() => !disabled && setIsOpen(!isOpen)}
          disabled={disabled}
        >
          <Icon name="calendar" size="sm" />
        </button>
      </div>

      {error && errorMessage && (
        <div
          style={{
            fontSize: "12px",
            color: tokens.color.semantic.error.value,
            marginTop: "4px",
          }}
        >
          {errorMessage}
        </div>
      )}

      {isOpen && (
        <div
          style={{
            position: "absolute",
            top: "100%",
            left: 0,
            right: 0,
            zIndex: 1000,
            background: "var(--color-background-primary)",
            border: `1px solid ${tokens.color.neutral[600].value}`,
            borderRadius: "8px",
            boxShadow: "0 4px 12px rgba(0, 0, 0, 0.3)",
            marginTop: "4px",
            padding: "16px",
            minWidth: "280px",
          }}
        >
          {/* Calendar Header */}
          <div
            style={{
              display: "flex",
              alignItems: "center",
              justifyContent: "space-between",
              marginBottom: "16px",
            }}
          >
            <Button
              variant="secondary"
              onClick={() => navigateMonth("prev")}
              style={{ padding: "4px 8px" }}
            >
              <Icon name="chevron-left" size="sm" />
            </Button>

            <div
              style={{
                fontSize: "16px",
                fontWeight: 600,
                color: "#FFFFFF",
                fontFamily: "var(--font-heading)",
              }}
            >
              {currentMonth.toLocaleDateString("en-US", {
                month: "long",
                year: "numeric",
              })}
            </div>

            <Button
              variant="secondary"
              onClick={() => navigateMonth("next")}
              style={{ padding: "4px 8px" }}
            >
              <Icon name="chevron-right" size="sm" />
            </Button>
          </div>

          {/* Calendar Days */}
          <div
            style={{
              display: "grid",
              gridTemplateColumns: "repeat(7, 1fr)",
              gap: "4px",
              marginBottom: "16px",
            }}
          >
            {/* Day headers */}
            {["Su", "Mo", "Tu", "We", "Th", "Fr", "Sa"].map((day) => (
              <div
                key={day}
                style={{
                  textAlign: "center",
                  fontSize: "12px",
                  fontWeight: 600,
                  color: tokens.color.neutral[400].value,
                  padding: "8px 4px",
                }}
              >
                {day}
              </div>
            ))}

            {/* Calendar days */}
            {generateCalendarDays().map((day, index) => (
              <button
                key={index}
                type="button"
                style={{
                  width: "32px",
                  height: "32px",
                  border: "none",
                  borderRadius: "4px",
                  background: day.isSelected
                    ? tokens.color.brand[600].value
                    : day.isToday
                      ? tokens.color.brand[600].value + "40"
                      : "transparent",
                  color:
                    day.isSelected || day.isToday
                      ? "#000000"
                      : day.isCurrentMonth
                        ? "#FFFFFF"
                        : tokens.color.neutral[400].value,
                  cursor: day.isDisabled ? "not-allowed" : "pointer",
                  fontSize: "14px",
                  fontWeight: day.isToday ? 600 : 400,
                  opacity: day.isDisabled ? 0.3 : 1,
                  display: "flex",
                  alignItems: "center",
                  justifyContent: "center",
                  transition: "all 0.2s ease",
                }}
                onClick={() => !day.isDisabled && handleDateSelect(day.date)}
                disabled={day.isDisabled}
                onMouseEnter={(e) => {
                  if (!day.isDisabled && !day.isSelected) {
                    e.currentTarget.style.background =
                      tokens.color.brand[600].value + "20";
                  }
                }}
                onMouseLeave={(e) => {
                  if (!day.isDisabled && !day.isSelected) {
                    e.currentTarget.style.background = day.isToday
                      ? tokens.color.brand[600].value + "40"
                      : "transparent";
                  }
                }}
              >
                {day.date.getDate()}
              </button>
            ))}
          </div>

          {/* Time picker */}
          {showTime && (
            <div
              style={{
                borderTop: `1px solid ${tokens.color.neutral[600].value}`,
                paddingTop: "16px",
              }}
            >
              <div
                style={{
                  fontSize: "14px",
                  fontWeight: 500,
                  color: "#FFFFFF",
                  marginBottom: "12px",
                }}
              >
                Time
              </div>

              <div
                style={{
                  display: "flex",
                  alignItems: "center",
                  gap: "8px",
                }}
              >
                <select
                  value={selectedTime.hours}
                  onChange={(e) =>
                    handleTimeChange("hours", parseInt(e.target.value))
                  }
                  style={{
                    background: "var(--color-background-secondary)",
                    border: `1px solid ${tokens.color.neutral[600].value}`,
                    borderRadius: "4px",
                    color: "#FFFFFF",
                    padding: "8px",
                    fontSize: "14px",
                  }}
                >
                  {Array.from(
                    { length: timeFormat === "12h" ? 12 : 24 },
                    (_, i) => {
                      const hour = timeFormat === "12h" ? i + 1 : i;
                      return (
                        <option key={hour} value={hour}>
                          {hour.toString().padStart(2, "0")}
                        </option>
                      );
                    },
                  )}
                </select>

                <span style={{ color: tokens.color.neutral[400].value }}>
                  :
                </span>

                <select
                  value={selectedTime.minutes}
                  onChange={(e) =>
                    handleTimeChange("minutes", parseInt(e.target.value))
                  }
                  style={{
                    background: "var(--color-background-secondary)",
                    border: `1px solid ${tokens.color.neutral[600].value}`,
                    borderRadius: "4px",
                    color: "#FFFFFF",
                    padding: "8px",
                    fontSize: "14px",
                  }}
                >
                  {Array.from({ length: 60 }, (_, i) => (
                    <option key={i} value={i}>
                      {i.toString().padStart(2, "0")}
                    </option>
                  ))}
                </select>

                {timeFormat === "12h" && (
                  <select
                    value={selectedTime.ampm}
                    onChange={(e) => handleTimeChange("ampm", e.target.value)}
                    style={{
                      background: "var(--color-background-secondary)",
                      border: `1px solid ${tokens.color.neutral[600].value}`,
                      borderRadius: "4px",
                      color: "#FFFFFF",
                      padding: "8px",
                      fontSize: "14px",
                    }}
                  >
                    <option value="AM">AM</option>
                    <option value="PM">PM</option>
                  </select>
                )}
              </div>
            </div>
          )}

          {/* Action buttons */}
          <div
            style={{
              display: "flex",
              justifyContent: "flex-end",
              gap: "8px",
              marginTop: "16px",
              paddingTop: "16px",
              borderTop: `1px solid ${tokens.color.neutral[600].value}`,
            }}
          >
            <Button
              variant="secondary"
              onClick={() => {
                onChange?.(null);
                setInputValue("");
                setIsOpen(false);
              }}
            >
              Clear
            </Button>
            <Button variant="primary" onClick={() => setIsOpen(false)}>
              Done
            </Button>
          </div>
        </div>
      )}
    </div>
  );
}
