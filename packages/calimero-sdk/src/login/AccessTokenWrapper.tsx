import React, { useCallback, useEffect } from 'react';
import { getAccessToken, getRefreshToken } from '../storage';
import { jwtDecode } from 'jwt-decode';
import { getNewJwtToken } from './refreshToken';

interface AccessTokenWrapperProps {
  children: React.ReactNode;
  getNodeUrl: () => string;
}

export const AccessTokenWrapper: React.FC<AccessTokenWrapperProps> = ({
  children,
  getNodeUrl,
}) => {
  const decodeToken = useCallback((token: string) => {
    try {
      return jwtDecode(token);
    } catch (error) {
      return null;
    }
  }, []);

  const isTokenExpiringSoon = useCallback((token: string) => {
    const decodedToken = decodeToken(token);
    if (!decodedToken || !decodedToken.exp) {
      return true;
    }

    const currentTime = Math.floor(Date.now() / 1000);
    const timeUntilExpiry = decodedToken.exp - currentTime;

    return timeUntilExpiry <= 5 * 60;
  }, [decodeToken]);

  const validateAccessToken = useCallback(async () => {
    const accessToken = getAccessToken();
    const refreshToken = getRefreshToken();

    if (!accessToken || !refreshToken) {
      return;
    }

    if (isTokenExpiringSoon(accessToken)) {
      try {
        await getNewJwtToken({ refreshToken, getNodeUrl });
      } catch (error) {
        console.log(error);
      }
    }
  }, [getNodeUrl, isTokenExpiringSoon]);

  useEffect(() => {
    validateAccessToken();

    const intervalId = setInterval(() => {
      validateAccessToken();
    }, 20 * 60 * 1000);

    return () => clearInterval(intervalId);
  }, [validateAccessToken]);

  return <>{children}</>;
};
