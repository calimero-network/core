import React, { useEffect, useState } from 'react';
import AppLoginPopup from '../components/login/applicationLogin/AppLoginPopup';
import { useServerDown } from './ServerDownContext';
import {
  closeLoginPopup,
  getStorageApplicationId,
  getStorageCallbackUrl,
  getStorageNodeAuthorized,
  setStorageApplicationId,
  setStorageCallbackUrl,
} from '../auth/storage';

interface AppLoginProviderProps {
  children: React.ReactNode;
}

interface AuthorizedPopupProps {
  applicationIdParam: string | null;
  callbackParam: string | null;
  storageApplicationId: string | null;
  storageCallbackUrl: string | null;
  setApplicationId: (value: string) => void;
  setCallbackUrl: (value: string) => void;
  setShowPopup: (value: boolean) => void;
}

interface UnAuthorizedPopupProps {
  applicationIdParam: string | null;
  callbackParam: string | null;
  setStorageApplicationId: (value: string) => void;
  setStorageCallbackUrl: (value: string) => void;
}

const AppLoginProvider = ({ children }: AppLoginProviderProps) => {
  const { showServerDownPopup } = useServerDown();
  const [applicationId, setApplicationId] = useState('');
  const [callbackUrl, setCallbackUrl] = useState('');
  const [showPopup, setShowPopup] = useState(false);

  const getUrlParams = () => {
    const urlParams = new URLSearchParams(window.location.search);
    return {
      applicationId: urlParams.get('application_id'),
      callbackUrl: decodeURIComponent(urlParams.get('callback_url') ?? ''),
    };
  };

  const handleAuthorizedNode = ({
    applicationIdParam,
    callbackParam,
    storageApplicationId,
    storageCallbackUrl,
    setApplicationId,
    setCallbackUrl,
    setShowPopup,
  }: AuthorizedPopupProps) => {
    if (applicationIdParam && callbackParam) {
      setApplicationId(applicationIdParam);
      setCallbackUrl(callbackParam);
      setShowPopup(true);
    } else if (storageApplicationId && storageCallbackUrl) {
      setApplicationId(storageApplicationId);
      setCallbackUrl(storageCallbackUrl);
      setShowPopup(true);
    }
  };

  const handleUnauthorizedNode = ({
    applicationIdParam,
    callbackParam,
    setStorageApplicationId,
    setStorageCallbackUrl,
  }: UnAuthorizedPopupProps) => {
    if (applicationIdParam && callbackParam) {
      setStorageApplicationId(applicationIdParam);
      setStorageCallbackUrl(callbackParam);
    }
  };

  useEffect(() => {
    const setupLoginPopup = () => {
      try {
        const {
          applicationId: applicationIdParam,
          callbackUrl: callbackParam,
        } = getUrlParams();
        const isNodeAuthorized = getStorageNodeAuthorized();
        const storageApplicationId = getStorageApplicationId();
        const storageCallbackUrl = getStorageCallbackUrl();

        if (isNodeAuthorized) {
          handleAuthorizedNode({
            applicationIdParam,
            callbackParam,
            storageApplicationId,
            storageCallbackUrl,
            setApplicationId,
            setCallbackUrl,
            setShowPopup,
          });
        } else {
          handleUnauthorizedNode({
            applicationIdParam,
            callbackParam,
            setStorageApplicationId,
            setStorageCallbackUrl,
          });
        }
      } catch (e) {
        console.error(e);
      }
    };
    setupLoginPopup();

    const originalPushState = window.history.pushState;

    window.history.pushState = function (...args) {
      originalPushState.apply(window.history, args);
      setupLoginPopup();
    };

    return () => {
      window.history.pushState = originalPushState;
    };
  }, []);

  const closePopup = () => {
    setShowPopup(false);
    closeLoginPopup();
  };

  return (
    <div>
      {showPopup && (
        <AppLoginPopup
          showPopup={showPopup}
          callbackUrl={callbackUrl}
          applicationId={applicationId}
          showServerDownPopup={showServerDownPopup}
          closePopup={closePopup}
        />
      )}
      {children}
    </div>
  );
};

export default AppLoginProvider;
