import { WalletType } from "../nodeApi";

export const getWalletType = (chainId: string): WalletType => {
  switch (chainId) {
    case "0x1":
      return WalletType.ETH({ chainId: 1 });
    case "0x38":
      return WalletType.ETH({ chainId: 56 });
    case "0xa4b1":
      return WalletType.ETH({ chainId: 42161 });
    case "0x144":
      return WalletType.ETH({ chainId: 324 });
    default:
      return WalletType.ETH({ chainId: 1 });
  }
};
