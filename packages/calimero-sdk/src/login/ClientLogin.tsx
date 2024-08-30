import React, { useEffect, useState } from 'react';
import { setAccessToken, setRefreshToken } from '../storage/storage';

interface ClientLoginProps {
  getNodeUrl: () => string | null;
  getApplicationId: () => string | null;
  sucessRedirect: () => void;
}

export const ClientLogin: React.FC<ClientLoginProps> = ({
  getApplicationId,
  getNodeUrl,
  sucessRedirect,
}) => {
  const [errorMessage, setErrorMessage] = useState<string>('');
  const redirectToDashboardLogin = () => {
    const nodeUrl = getNodeUrl();
    const applicationId = getApplicationId();
    if (!nodeUrl) {
      setErrorMessage('Node URL is not set');
      return;
    }
    if (!applicationId) {
      setErrorMessage('Application ID is not set');
      return;
    }

    const callbackUrl = encodeURIComponent(window.location.href);
    const redirectUrl = `${nodeUrl}/admin-dashboard/?application_id=${applicationId}&callback_url=${callbackUrl}`;

    window.location.href = redirectUrl;
  };

  useEffect(() => {
    const urlParams = new URLSearchParams(window.location.search);
    const encodedAccessToken = urlParams.get('access_token');
    const encodedRefreshToken = urlParams.get('refresh_token');
    if (encodedAccessToken && encodedRefreshToken) {
      const accessToken = decodeURIComponent(encodedAccessToken);
      const refreshToken = decodeURIComponent(encodedRefreshToken);
      setAccessToken(accessToken);
      setRefreshToken(refreshToken);
      sucessRedirect();
    }
  }, []);

  return (
    <div
      style={{
        display: 'flex',
        flexDirection: 'column',
        alignItems: 'center',
        padding: '0.5rem',
        maxWidth: '400px',
      }}
    >
      <div
        style={{
          marginTop: '1.5rem',
          display: 'grid',
          color: 'white',
          fontSize: '1.25rem',
          fontWeight: '500',
          textAlign: 'center',
        }}
      >
        <span
          style={{
            marginBottom: '0.5rem',
            color: '#fff',
          }}
        >
          Login with Admin Dashboard
        </span>
      </div>
      <button
        style={{
          backgroundColor: '#FF7A00',
          color: 'white',
          width: '100%',
          display: 'flex',
          justifyContent: 'center',
          alignItems: 'center',
          gap: '0.5rem',
          height: '46px',
          cursor: 'pointer',
          fontSize: '1rem',
          fontWeight: '500',
          borderRadius: '0.375rem',
          border: 'none',
          outline: 'none',
          paddingLeft: '0.5rem',
          paddingRight: '0.5rem',
        }}
        onClick={redirectToDashboardLogin}
      >
        Login
      </button>
      <div
        style={{
          color: '#ef4444',
          fontSize: '0.875rem',
          marginTop: '0.5rem',
        }}
      >
        {errorMessage}
      </div>
    </div>
  );
};
