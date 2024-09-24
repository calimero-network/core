export const copyToClipboard = (text: string) => {
  navigator.clipboard.writeText(text).catch((err) => {
    console.error('Failed to copy text to clipboard: ', err);
  });
};
