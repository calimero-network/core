export const truncatePublicKey = (publicKey: string): string => {
  const keyValue = publicKey?.split(":")[1] ?? "";

  if (keyValue) {
    return `${keyValue.substring(0, 4)}...${keyValue.substring(
      keyValue.length - 4,
      keyValue.length
    )}`;
  } else {
    return "";
  }
};

export const truncateHash = (hash: string): string => {
  return `
      ${hash.substring(0, 4)}...${hash.substring(
    hash.length - 4,
    hash.length
  )}`;
};

export const getStatus = (active: boolean, revoked: boolean): string => {
  if (active) {
    return "active";
  } else if (revoked) {
    return "revoked";
  } else {
    return "";
  }
};
