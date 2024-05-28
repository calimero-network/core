export function formatTimestampToDate(timestamp: number): string {
  const date = new Date(timestamp);

  const formatter = new Intl.DateTimeFormat(undefined, {
    year: 'numeric',
    month: '2-digit',
    day: '2-digit'
  });

  return formatter.format(date);
}
