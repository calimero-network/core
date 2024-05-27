import { WalletType } from "../nodeApi";

export const getWalletType = (chainId: string): WalletType => {
  switch (chainId) {
    case "0x1":
      return WalletType.ETH;
    case "0x38":
      return WalletType.BNB;
    case "0xa4b1":
      return WalletType.ARB;
    case "0x144":
      return WalletType.ZK;
    default:
      return WalletType.ETH;
  }
};
