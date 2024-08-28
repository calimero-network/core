import { useCallback, useEffect, useState } from 'react';
import apiClient from '../api';
import React from 'react';
import Spinner from '../components/loader/Spinner';

export interface SetupModalProps {
  successRoute: () => void;
  getNodeUrl: () => string | null;
  setNodeUrl: (url: string) => void;
  getApplicationId?: () => string | null;
  setApplicationId?: (applicationId: string) => void;
}

export const SetupModal: React.FC<SetupModalProps> = (
  props: SetupModalProps,
) => {
  const [error, setError] = useState<string | null>(null);
  const [applicationError, setApplicationError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [url, setUrl] = useState<string | null>(null);
  const [applicationId, setApplicationId] = useState<string | null>(null);
  const MINIMUM_LOADING_TIME_MS = 1000;

  useEffect(() => {
    setUrl(props.getNodeUrl());
    if (props.getApplicationId) {
      setApplicationId(props.getApplicationId());
    }
  }, [props]);

  function validateUrl(value: string): boolean {
    try {
      new URL(value);
      return true;
    } catch (e) {
      return false;
    }
  }

  function validateContext(value: string) {
    if (value.length < 32 || value.length > 44) {
      setApplicationError('Application ID must be between 32 and 44 characters long.');
      return;
    }
    const validChars =
      /^[123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz]+$/;

    if (!validChars.test(value)) {
      setApplicationError('Application ID must contain only base58 characters.');
      return;
    }
  }

  const handleChange = (url: string) => {
    setError('');
    setUrl(url);
  };

  const handleChangeContextId = (value: string) => {
    setApplicationError('');
    setApplicationId(value);
    validateContext(value);
  };

  const checkConnection = useCallback(async () => {
    if (!url) return;
    if (validateUrl(url.toString())) {
      setLoading(true);
      const timer = new Promise((resolve) =>
        setTimeout(resolve, MINIMUM_LOADING_TIME_MS),
      );

      const fetchData = apiClient.node().health({ url: url });
      Promise.all([timer, fetchData]).then(([, response]) => {
        if (response.data) {
          setError('');
          props.setNodeUrl(url);
          props.setApplicationId && props.setApplicationId(applicationId || '');
          props.successRoute();
        } else {
          setError('Connection failed. Please check if node url is correct.');
        }
        setLoading(false);
      });
    } else {
      setError('Connection failed. Please check if node url is correct.');
    }
  }, [props, url, applicationId]);

  const disableButton = (): boolean => {
    if (!url) return true;
    if (props.getApplicationId && props.setApplicationId) {
      if (applicationError) return true;
      if (!applicationId) return true;
    }
    return false;
  };

  return (
    <div
      style={{
        display: 'flex',
        height: '100vh',
        justifyContent: 'center',
        backgroundColor: '#111111',
      }}
    >
      <div
        style={{
          display: 'flex',
          flexDirection: 'column',
          justifyContent: 'center',
          alignItems: 'center',
        }}
      >
        <div
          style={{
            display: 'flex',
            flexDirection: 'column',
            alignItems: 'center',
            backgroundColor: '#1c1c1c',
            padding: '2rem',
            gap: '1rem',
            borderRadius: '0.5rem',
          }}
        >
          <div
            style={{
              display: 'flex',
              flexDirection: 'column',
              justifyContent: 'center',
              alignItems: 'center',
              gap: '2rem',
              padding: '0 3.5rem',
            }}
          >
            <div
              style={{
                color: 'white',
                fontSize: '2.5rem',
                fontWeight: 600,
              }}
            >
              App setup
            </div>
            {loading ? (
              <Spinner />
            ) : (
              <>
                <div
                  style={{
                    display: 'flex',
                    flexDirection: 'column',
                    gap: '0.5rem',
                  }}
                >
                  {props.setApplicationId && props.getApplicationId && (
                    <>
                      <input
                        type="text"
                        placeholder="application id"
                        value={applicationId?.toString() || ''}
                        onChange={(e: { target: { value: string } }) => {
                          handleChangeContextId(e.target.value);
                        }}
                        style={{
                          width: '400px',
                          padding: '0.5rem',
                          borderRadius: '0.375rem',
                        }}
                      />
                      <div
                        style={{
                          color: '#ef4444',
                          fontSize: '0.875rem',
                        }}
                      >
                        {applicationError}
                      </div>
                    </>
                  )}
                  <input
                    type="text"
                    placeholder="node url"
                    inputMode="url"
                    value={url?.toString() || ''}
                    onChange={(e: { target: { value: string } }) => {
                      handleChange(e.target.value);
                    }}
                    style={{
                      width: '400px',
                      padding: '0.5rem',
                      borderRadius: '0.375rem',
                    }}
                  />
                  <div
                    style={{
                      color: '#ef4444',
                      fontSize: '0.875rem',
                    }}
                  >
                    {error}
                  </div>
                  <button
                    style={{
                      backgroundColor: '#6b7280',
                      color: 'white',
                      width: '100%',
                      display: 'flex',
                      justifyContent: 'center',
                      alignItems: 'center',
                      gap: '0.5rem',
                      height: '46px',
                      fontSize: '1rem',
                      fontWeight: 500,
                      borderRadius: '0.375rem',
                      border: 'none',
                      outline: 'none',
                      padding: '0.5rem',
                      cursor: disableButton() ? 'not-allowed' : 'pointer',
                    }}
                    disabled={disableButton()}
                    onClick={() => {
                      checkConnection();
                    }}
                  >
                    <span>Set values</span>
                  </button>
                </div>
              </>
            )}
          </div>
        </div>
      </div>
    </div>
  );
};
