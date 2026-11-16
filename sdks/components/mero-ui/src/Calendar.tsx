import React, { useState, useMemo } from "react";
import { tokens } from "@calimero-network/mero-tokens";
import { Icon } from "./Icon";

interface CalendarProps {
  value?: Date;
  onChange?: (date: Date) => void;
  minDate?: Date;
  maxDate?: Date;
  disabled?: boolean;
  size?: "sm" | "md" | "lg";
  variant?: "default" | "filled" | "outlined";
  showWeekNumbers?: boolean;
  className?: string;
  style?: React.CSSProperties;
}

export const Calendar: React.FC<CalendarProps> = ({
  value,
  onChange,
  minDate,
  maxDate,
  disabled = false,
  size = "md",
  variant = "default",
  showWeekNumbers = false,
  className = "",
  style = {},
}) => {
  const [currentDate, setCurrentDate] = useState(value || new Date());
  const [viewMode, setViewMode] = useState<"month" | "year">("month");

  const sizeStyles = {
    sm: {
      fontSize: "12px",
      cellSize: "28px",
      headerHeight: "32px",
    },
    md: {
      fontSize: "14px",
      cellSize: "36px",
      headerHeight: "40px",
    },
    lg: {
      fontSize: "16px",
      cellSize: "44px",
      headerHeight: "48px",
    },
  };

  const variantStyles = {
    default: {
      backgroundColor: tokens.color.background.primary.value,
      border: `1px solid ${tokens.color.neutral[600].value}`,
    },
    filled: {
      backgroundColor: tokens.color.background.tertiary.value,
      border: `1px solid ${disabled ? tokens.color.neutral[400].value : "transparent"}`,
    },
    outlined: {
      backgroundColor: "transparent",
      border: `2px solid ${tokens.color.neutral[600].value}`,
    },
  };

  const calendarData = useMemo(() => {
    const year = currentDate.getFullYear();
    const month = currentDate.getMonth();

    const firstDay = new Date(year, month, 1);
    const lastDay = new Date(year, month + 1, 0);
    const startDate = new Date(firstDay);
    startDate.setDate(startDate.getDate() - firstDay.getDay());

    const weeks: Date[][] = [];
    let currentWeek: Date[] = [];

    for (let i = 0; i < 42; i++) {
      const date = new Date(startDate);
      date.setDate(startDate.getDate() + i);
      currentWeek.push(date);

      if (currentWeek.length === 7) {
        weeks.push(currentWeek);
        currentWeek = [];
      }
    }

    return { weeks, year, month };
  }, [currentDate]);

  const isDateDisabled = (date: Date): boolean => {
    if (disabled) return true;
    if (minDate && date < minDate) return true;
    if (maxDate && date > maxDate) return true;
    return false;
  };

  const isDateSelected = (date: Date): boolean => {
    if (!value) return false;
    return (
      date.getDate() === value.getDate() &&
      date.getMonth() === value.getMonth() &&
      date.getFullYear() === value.getFullYear()
    );
  };

  const isToday = (date: Date): boolean => {
    const today = new Date();
    return (
      date.getDate() === today.getDate() &&
      date.getMonth() === today.getMonth() &&
      date.getFullYear() === today.getFullYear()
    );
  };

  const isCurrentMonth = (date: Date): boolean => {
    return date.getMonth() === calendarData.month;
  };

  const handleDateClick = (date: Date) => {
    if (isDateDisabled(date)) return;
    onChange?.(date);
  };

  const handlePrevMonth = () => {
    setCurrentDate(
      new Date(currentDate.getFullYear(), currentDate.getMonth() - 1, 1),
    );
  };

  const handleNextMonth = () => {
    setCurrentDate(
      new Date(currentDate.getFullYear(), currentDate.getMonth() + 1, 1),
    );
  };

  const handlePrevYear = () => {
    setCurrentDate(
      new Date(currentDate.getFullYear() - 1, currentDate.getMonth(), 1),
    );
  };

  const handleNextYear = () => {
    setCurrentDate(
      new Date(currentDate.getFullYear() + 1, currentDate.getMonth(), 1),
    );
  };

  const monthNames = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
  ];

  const dayNames = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

  const calendarStyle: React.CSSProperties = {
    ...variantStyles[variant],
    borderRadius: tokens.radius.md.value,
    padding: tokens.space["4"].value,
    width: "fit-content",
    ...style,
  };

  const headerStyle: React.CSSProperties = {
    display: "flex",
    alignItems: "center",
    justifyContent: "space-between",
    marginBottom: tokens.space["4"].value,
    height: sizeStyles[size].headerHeight,
  };

  const navButtonStyle: React.CSSProperties = {
    display: "flex",
    alignItems: "center",
    justifyContent: "center",
    width: sizeStyles[size].cellSize,
    height: sizeStyles[size].cellSize,
    border: "none",
    backgroundColor: "transparent",
    color: tokens.color.background.primary.value,
    cursor: "pointer",
    borderRadius: tokens.radius.sm.value,
    transition: "background-color 0.2s ease",
  };

  const titleStyle: React.CSSProperties = {
    fontSize: sizeStyles[size].fontSize,
    fontWeight: 600,
    color: tokens.color.background.primary.value,
    cursor: "pointer",
    padding: `${tokens.space["2"].value} ${tokens.space["3"].value}`,
    borderRadius: tokens.radius.sm.value,
    transition: "background-color 0.2s ease",
  };

  const weekHeaderStyle: React.CSSProperties = {
    display: "grid",
    gridTemplateColumns: showWeekNumbers
      ? "auto repeat(7, 1fr)"
      : "repeat(7, 1fr)",
    gap: "2px",
    marginBottom: tokens.space["2"].value,
  };

  const dayHeaderStyle: React.CSSProperties = {
    display: "flex",
    alignItems: "center",
    justifyContent: "center",
    height: sizeStyles[size].cellSize,
    fontSize: sizeStyles[size].fontSize,
    fontWeight: 500,
    color: tokens.color.neutral[300].value,
  };

  const weekStyle: React.CSSProperties = {
    display: "grid",
    gridTemplateColumns: showWeekNumbers
      ? "auto repeat(7, 1fr)"
      : "repeat(7, 1fr)",
    gap: "2px",
  };

  const cellStyle: React.CSSProperties = {
    display: "flex",
    alignItems: "center",
    justifyContent: "center",
    width: sizeStyles[size].cellSize,
    height: sizeStyles[size].cellSize,
    fontSize: sizeStyles[size].fontSize,
    cursor: "pointer",
    borderRadius: tokens.radius.sm.value,
    transition: "all 0.2s ease",
  };

  const weekNumberStyle: React.CSSProperties = {
    display: "flex",
    alignItems: "center",
    justifyContent: "center",
    fontSize: sizeStyles[size].fontSize,
    color: tokens.color.neutral[300].value,
    fontWeight: 500,
  };

  return (
    <div className={className} style={calendarStyle}>
      <div style={headerStyle}>
        <button
          style={navButtonStyle}
          onClick={viewMode === "month" ? handlePrevMonth : handlePrevYear}
          onMouseEnter={(e) => {
            e.currentTarget.style.backgroundColor =
              tokens.color.background.brand.value;
          }}
          onMouseLeave={(e) => {
            e.currentTarget.style.backgroundColor = "transparent";
          }}
        >
          <Icon name="chevron-left" size="sm" color="current" />
        </button>

        <div
          style={titleStyle}
          onClick={() => setViewMode(viewMode === "month" ? "year" : "month")}
          onMouseEnter={(e) => {
            e.currentTarget.style.backgroundColor =
              tokens.color.background.brand.value;
          }}
          onMouseLeave={(e) => {
            e.currentTarget.style.backgroundColor = "transparent";
          }}
        >
          {viewMode === "month"
            ? `${monthNames[calendarData.month]} ${calendarData.year}`
            : calendarData.year}
        </div>

        <button
          style={navButtonStyle}
          onClick={viewMode === "month" ? handleNextMonth : handleNextYear}
          onMouseEnter={(e) => {
            e.currentTarget.style.backgroundColor =
              tokens.color.background.brand.value;
          }}
          onMouseLeave={(e) => {
            e.currentTarget.style.backgroundColor = "transparent";
          }}
        >
          <Icon name="chevron-right" size="sm" color="current" />
        </button>
      </div>

      <div style={weekHeaderStyle}>
        {showWeekNumbers && <div style={dayHeaderStyle}>Wk</div>}
        {dayNames.map((day) => (
          <div key={day} style={dayHeaderStyle}>
            {day}
          </div>
        ))}
      </div>

      {calendarData.weeks.map((week, weekIndex) => (
        <div key={weekIndex} style={weekStyle}>
          {showWeekNumbers && (
            <div style={weekNumberStyle}>
              {Math.ceil((week[0].getDate() - week[0].getDay()) / 7) + 1}
            </div>
          )}
          {week.map((date, dayIndex) => {
            const isSelected = isDateSelected(date);
            const isTodayDate = isToday(date);
            const isDisabled = isDateDisabled(date);
            const isCurrentMonthDate = isCurrentMonth(date);

            return (
              <div
                key={dayIndex}
                style={{
                  ...cellStyle,
                  backgroundColor: isSelected
                    ? tokens.color.brand[600].value
                    : isTodayDate
                      ? tokens.color.background.brand.value
                      : "transparent",
                  color: isSelected
                    ? tokens.color.neutral[200].value
                    : isCurrentMonthDate
                      ? tokens.color.background.primary.value
                      : tokens.color.neutral[300].value,
                  cursor: isDisabled ? "not-allowed" : "pointer",
                  opacity: isDisabled ? 0.5 : 1,
                  fontWeight: isTodayDate ? 600 : 400,
                }}
                onClick={() => handleDateClick(date)}
                onMouseEnter={(e) => {
                  if (!isDisabled && !isSelected) {
                    e.currentTarget.style.backgroundColor =
                      tokens.color.background.brand.value;
                  }
                }}
                onMouseLeave={(e) => {
                  if (!isDisabled && !isSelected) {
                    e.currentTarget.style.backgroundColor = "transparent";
                  }
                }}
              >
                {date.getDate()}
              </div>
            );
          })}
        </div>
      ))}
    </div>
  );
};
