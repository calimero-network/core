import { WalletType } from '../../api/nodeApi';

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
    default:
      return WalletType.ETH({ chainId: 1 });
  }
};

export const getWalletType = (walletType: string): WalletType => {
  switch (walletType) {
    case 'argentX':
      return WalletType.SN({ walletName: 'argentX' });
    default:
      return WalletType.SN({ walletName: 'metamask' });
  }
}
