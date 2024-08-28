export const truncatePublicKey = (publicKey: string): string => {
  const keyValue = publicKey?.split(':')[1] ?? '';

  if (keyValue) {
    return `${keyValue.substring(0, 4)}...${keyValue.substring(
      keyValue.length - 4,
      keyValue.length,
    )}`;
  } else {
    return '';
  }
};

export const truncateText = (text: string): string => {
  return `${text.substring(0, 4)}...${text.substring(
    text.length - 4,
    text.length,
  )}`;
};

export const truncateHash = (hash: string): string => {
  return `
      ${hash.substring(0, 4)}...${hash.substring(
        hash.length - 4,
        hash.length,
      )}`;
};

export const getStatus = (active: boolean, revoked: boolean): string => {
  if (active) {
    return 'active';
  } else if (revoked) {
    return 'revoked';
  } else {
    return '';
  }
};

export const convertBytes = (bytes: number): string => {
  if (bytes === 0) {
    return '0 MB';
  }

  const bytesInOneMB = 1024 * 1024;
  const bytesInOneGB = 1024 * 1024 * 1024;

  if (bytes < bytesInOneGB) {
    const mb = bytes / bytesInOneMB;
    return `${(Math.round(mb * 100) / 100).toFixed(2)} MB`;
  } else {
    const gb = bytes / bytesInOneGB;
    return `${(Math.round(gb * 100) / 100).toFixed(2)} GB`;
  }
};
