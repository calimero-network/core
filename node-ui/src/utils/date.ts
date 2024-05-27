export function formatTimestampToDate(timestamp: number): string {
  const date = new Date(timestamp);

  const day = date.getDate();
  const month = date.getMonth() + 1;
  const year = date.getFullYear();

  const dayString = day < 10 ? "0" + day : day.toString();
  const monthString = month < 10 ? "0" + month : month.toString();

  return `${dayString}.${monthString}.${year}`;
}
