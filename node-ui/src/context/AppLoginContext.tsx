import React, { useEffect, useState } from 'react';
import AppLoginPopup from '../components/login/applicationLogin/AppLoginPopup';
import { useServerDown } from './ServerDownContext';

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
        console.log(applicationIdParam);
        console.log(callbackParam);
        console.log('params');
        if (applicationIdParam && callbackParam) {
          setApplicationId(applicationIdParam);
          setCallbackUrl(callbackParam);
          setShowPopup(true);
        }
      } catch (e) {
        console.error(e);
      }
    };
    setupLoginPopup();
  }, []);

  return (
    <div>
      <AppLoginPopup
        showPopup={showPopup}
        callbackUrl={callbackUrl}
        applicationId={applicationId}
        showServerDownPopup={showServerDownPopup}
      />
      {children}
    </div>
  );
};

export default AppLoginProvider;
