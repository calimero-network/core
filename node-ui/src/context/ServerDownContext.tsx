import React, { createContext, useContext, useState } from 'react';
import StatusModal from '../components/common/StatusModal';
import { getAppEndpointKey } from '../utils/storage';
import { HealthStatus } from '../api/dataSource/NodeDataSource';
import { ResponseData } from '../api/response';
import apiClient from '../api';
import translations from '../constants/en.global.json';
import { interpolate } from '../utils/interpolate';

const t = translations.serverDown;

interface ServerDownContextProps {
  showServerDownPopup: () => void;
  hideServerDownPopup: () => void;
}

const ServerDownContext = createContext<ServerDownContextProps | undefined>(
  undefined,
);

interface ServerDownProviderProps {
  children: React.ReactNode;
}

export const ServerDownProvider = ({ children }: ServerDownProviderProps) => {
  const [isPopupVisible, setIsPopupVisible] = useState(false);
  const nodeApiEndpoint = getAppEndpointKey();

  const showServerDownPopup = () => setIsPopupVisible(true);
  const hideServerDownPopup = () => setIsPopupVisible(false);

  const checkNodeApiEndpoint = async () => {
    const formattedNodeEndpoint: string = new URL(
      nodeApiEndpoint as string,
    ).href.replace(/\/$/, '');
    const response: ResponseData<HealthStatus> = await apiClient(
      showServerDownPopup,
    )
      .node()
      .health({ url: formattedNodeEndpoint });
    if (response.data) {
      showServerDownPopup();
    }
    hideServerDownPopup();
  };

  return (
    <ServerDownContext.Provider
      value={{ showServerDownPopup, hideServerDownPopup }}
    >
      {children}
      <StatusModal
        closeModal={checkNodeApiEndpoint}
        modalContent={{
          title: t.popupTitle,
          message: interpolate(t.popupMessage, {
            nodeApiEndpoint: nodeApiEndpoint ?? '',
          }),
          error: true,
        }}
        show={isPopupVisible}
      />
    </ServerDownContext.Provider>
  );
};

export const useServerDown = () => {
  const context = useContext(ServerDownContext);
  if (!context) {
    throw new Error(t.useHookComponentErrorText);
  }
  return context;
};
