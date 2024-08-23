import React from 'react';

import { MetaMaskUIProvider } from '@metamask/sdk-react-ui';
import { Outlet } from 'react-router-dom';
import translations from '../../constants/en.global.json';

export default function MetamaskRoute() {
  const t = translations.useMetamask;

  return (
    <MetaMaskUIProvider
      sdkOptions={{
        dappMetadata: {
          name: t.applicationNameText,
        },
        checkInstallationOnAllCalls: true,
      }}
    >
      <Outlet />
    </MetaMaskUIProvider>
  );
}
