import React, { useEffect, useState } from 'react';
import AppLoginPopup from '../components/login/applicationLogin/AppLoginPopup';
import { useServerDown } from './ServerDownContext';
import { getStorageApplicationId, getStorageCallbackUrl, getStorageNodeAuthorized, setStorageApplicationId, setStorageCallbackUrl } from '../auth/storage';

interface AppLoginProviderProps {
  children: React.ReactNode;
}

const AppLoginProvider = ({ children }: AppLoginProviderProps) => {
  const { showServerDownPopup } = useServerDown();
  const [applicationId, setApplicationId] = useState('');
  const [callbackUrl, setCallbackUrl] = useState('');
  const [showPopup, setShowPopup] = useState(false);

  useEffect(() => {
    const setupLoginPopup = () => {
      try {
        const urlParams = new URLSearchParams(window.location.search);
        const applicationIdParam = urlParams.get('application_id');
        const callbackParam = decodeURIComponent(
          urlParams.get('callback_url') ?? '',
        );
        const isNodeAuthorized = getStorageNodeAuthorized();
        const storageApplicationId = getStorageApplicationId();
        const storageCallbackUrl = getStorageCallbackUrl();
        if (isNodeAuthorized) {
          if (applicationIdParam && callbackParam) {
            setApplicationId(applicationIdParam);
            setCallbackUrl(callbackParam);
            setShowPopup(true);
          } else if (storageApplicationId && storageCallbackUrl) {
            setApplicationId(storageApplicationId);
            setCallbackUrl(storageCallbackUrl);
            setShowPopup(true);
          }
        } else {
          if (applicationIdParam && callbackParam) {
            setStorageApplicationId(applicationIdParam);
            setStorageCallbackUrl(callbackParam);
          }
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

  return (
    <div>
      {showPopup && <AppLoginPopup
        showPopup={showPopup}
        callbackUrl={callbackUrl}
        applicationId={applicationId}
        showServerDownPopup={showServerDownPopup}
      />}
      {children}
    </div>
  );
};

export default AppLoginProvider;
