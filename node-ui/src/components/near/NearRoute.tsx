import React from 'react';

import { WalletSelectorContextProvider } from '../../context/WalletSelectorContext';
import { getNearEnvironment } from '../../utils/node';
import { Outlet } from 'react-router-dom';

export default function NearRoute() {
  return (
    <WalletSelectorContextProvider network={getNearEnvironment()}>
      <Outlet />
    </WalletSelectorContextProvider>
  );
}
