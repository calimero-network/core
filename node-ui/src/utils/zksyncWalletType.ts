import { WalletType } from '../api/dataSource/NodeDataSource';

export const getNetworkType = (chainId: string): WalletType => {
  switch (chainId) {
    // Mainnet
    case '0x144':
      return WalletType.ZKSYNC;
    // Testnet
    case '0x118':
      return WalletType.ZKSYNC;
    // Local
    case '0x12c':
      return WalletType.ZKSYNC;
    default:
      return WalletType.ZKSYNC;
  }
};

export const getNetworkName = (chainId: number): string => {
  switch (chainId) {
    case 324:
      return 'zkSync Mainnet';
    case 280:
      return 'zkSync Testnet';
    case 300:
      return 'zkSync Local';
    default:
      return 'Unknown zkSync Network';
  }
};

export const getNetworkRpcUrl = (chainId: number): string => {
  switch (chainId) {
    case 324:
      return 'https://mainnet.era.zksync.io';
    case 280:
      return 'https://testnet.era.zksync.dev';
    case 300:
      return 'http://localhost:3050';
    default:
      throw new Error(`Unsupported zkSync network: ${chainId}`);
  }
};

export const getNetworkExplorerUrl = (chainId: number): string => {
  switch (chainId) {
    case 324:
      return 'https://explorer.zksync.io';
    case 280:
      return 'https://goerli.explorer.zksync.io';
    case 300:
      return 'http://localhost:3010';
    default:
      throw new Error(`Unsupported zkSync network: ${chainId}`);
  }
};
