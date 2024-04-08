export const getDisplayPublicKey = (publicKey) => {
  return `
      ${publicKey.split(":")[1].substring(0, 4)}...${publicKey
    .split(":")[1]
    .substring(
      publicKey.split(":")[1].length - 4,
      publicKey.split(":")[1].length
    )}`;
};

export const getStatus = (active, revoked) => {
  if (active && !revoked) {
    return "active";
  } else if (revoked && !active) {
    return "revoked";
  } else {
    return "";
  }
};
