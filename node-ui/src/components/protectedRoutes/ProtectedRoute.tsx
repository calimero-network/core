import React, { useEffect } from 'react';
import { useNavigate, Outlet } from 'react-router-dom';
import { isNodeAuthorized, getClientKey } from '../../utils/storage';
import { getPathname } from '../../utils/protectedRoute';

export default function ProtectedRoute() {
  const navigate = useNavigate();
  const clientKey = getClientKey();
  const isAuthorized = isNodeAuthorized();
  const pathname = getPathname();

  useEffect(() => {
    const isAuthPath = pathname.startsWith('/auth');
    if (isAuthPath) {
      if (isAuthorized && clientKey) {
        //navigate to home page after auth is successfull
        navigate('/identity');
      }
    } else {
      if (!(isAuthorized && clientKey)) {
        //show auth if not authorized
        navigate('/auth');
      }
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [isAuthorized, clientKey]);

  return <Outlet />;
}
