import { WalletType } from '../api/dataSource/NodeDataSource';

export const getNetworkType = (chainId: string): WalletType => {
  switch (chainId) {
    // Mainnet
    case '0x144':
      return WalletType.ZKSYNC_MAINNET;
    // Testnet
    case '0x118':
      return WalletType.ZKSYNC_TESTNET;
    // Local
    case '0x12c':
      return WalletType.ZKSYNC_LOCAL;
    // Lens
    case '0x1':
      return WalletType.ZKSYNC_LENS;
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
    case 1:
      return 'Lens Protocol';
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
    case 1:
      return 'https://lens.xyz';
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
