import { WalletType } from '../api/dataSource/NodeDataSource';

export const getNetworkType = (chainId: string): WalletType => {
  switch (chainId) {
    case '0x1':
      return WalletType.ETH({ chainId: 1 });
    case '0x38':
      return WalletType.ETH({ chainId: 56 });
    case '0xa4b1':
      return WalletType.ETH({ chainId: 42161 });
    case '0x144':
      return WalletType.ETH({ chainId: 324 });
    case '0x118':
      return WalletType.ETH({ chainId: 280 });
    default:
      return WalletType.ETH({ chainId: 1 });
  }
};
