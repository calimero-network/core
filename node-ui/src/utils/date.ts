export function formatDateWithTime(timestamp: string): string {
  const date = new Date(Number(timestamp));

  const formatter = new Intl.DateTimeFormat(undefined, {
    year: 'numeric',
    month: 'short',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
    hour12: false,
  });

  return formatter.format(date);
}

export function formatTimestampToDate(timestamp: number): string {
  // Convert seconds to milliseconds
  const date = new Date(timestamp * 1000);

  const formatter = new Intl.DateTimeFormat(undefined, {
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
  });

  return formatter.format(date);
}
