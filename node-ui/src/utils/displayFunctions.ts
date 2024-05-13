export const truncatePublicKey = (publicKey: string): string => {
  return `
      ${publicKey.split(":")[1].substring(0, 4)}...${publicKey
    .split(":")[1]
    .substring(
      publicKey.split(":")[1].length - 4,
      publicKey.split(":")[1].length
    )}`;
};

export const truncateHash = (hash: string): string => {
  return `
      ${hash.substring(0, 4)}...${hash
    .substring(
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
