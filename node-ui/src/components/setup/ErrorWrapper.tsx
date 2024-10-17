import React, { useEffect, useState } from 'react';
import { useLocation, useNavigate } from 'react-router-dom';

import StatusModal from '../common/StatusModal';
import translations from '../../constants/en.global.json';

export default function ErrorWrapper({
  children,
}: {
  children: React.ReactNode;
}) {
  const t = translations.setupModal;
  const navigate = useNavigate();
  const location = useLocation();
  const [isPopupVisible, setIsPopupVisible] = useState(false);

  useEffect(() => {
    const params = new URLSearchParams(location.search);
    if (params.get('node_error') === 'true') {
      setIsPopupVisible(true);
    }
  }, [location.search]);

  return (
    <>
      {children}
      {isPopupVisible && (
        <StatusModal
          closeModal={() => {
            navigate('/');
            setIsPopupVisible(false);
          }}
          modalContent={{
            title: t.errorTitle,
            message: t.errorMessage,
            error: true,
          }}
          show={isPopupVisible}
        />
      )}
    </>
  );
}
