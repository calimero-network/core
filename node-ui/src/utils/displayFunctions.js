export const truncatePublicKey = (publicKey) => {
  return `
      ${publicKey.split(":")[1].substring(0, 4)}...${publicKey
    .split(":")[1]
    .substring(
      publicKey.split(":")[1].length - 4,
      publicKey.split(":")[1].length
    )}`;
};

export const getStatus = (active, revoked) => {
  if (active) {
    return "active";
  } else if (revoked) {
    return "revoked";
  } else {
    return "";
  }
};
