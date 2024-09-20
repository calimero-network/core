import { useCallback, useEffect, useState } from 'react';
import React from 'react';
import apiClient from '../../api';
import LoaderSpinner from '../common/LoaderSpinner';
import translations from '../../constants/en.global.json';

export interface SetupModalProps {
  successRoute: () => void;
  setNodeUrl: (url: string) => void;
  getNodeUrl: () => string;
}

export function SetupModal({
  successRoute,
  setNodeUrl,
  getNodeUrl,
}: SetupModalProps) {
  const t = translations.setupModal;
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [url, setUrl] = useState<string | null>(null);
  const MINIMUM_LOADING_TIME_MS = 1000;

  useEffect(() => {
    setUrl(getNodeUrl());
  }, []);

  function validateUrl(value: string): boolean {
    try {
      new URL(value);
      return true;
    } catch (e) {
      return false;
    }
  }

  const handleChange = (url: string) => {
    setError('');
    setUrl(url);
  };

  const checkConnection = useCallback(async () => {
    if (!url) return;
    if (validateUrl(url.toString())) {
      setLoading(true);
      const timer = new Promise((resolve) =>
        setTimeout(resolve, MINIMUM_LOADING_TIME_MS),
      );
      try {
        const fetchData = apiClient(() => {
          setError(t.nodeHealthCheckError);
          setLoading(false);
        })
          .node()
          .health({ url: url });
        Promise.all([timer, fetchData]).then(([, response]) => {
          if (response.data) {
            setError('');
            setNodeUrl(url);
            successRoute();
          } else {
            setError(t.nodeHealthCheckError);
          }
          setLoading(false);
        });
      } catch (error) {
        console.log(error);
        setError(t.nodeHealthCheckError);
        setLoading(false);
      }
    } else {
      setError(t.nodeHealthCheckError);
    }
  }, [setNodeUrl, successRoute, t.nodeHealthCheckError, url]);

  const isDisabled = (): boolean => {
    if (!url) return true;
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
              {t.modalTitle}
            </div>
            {loading ? (
              <LoaderSpinner />
            ) : (
              <>
                <div
                  style={{
                    display: 'flex',
                    flexDirection: 'column',
                    gap: '0.5rem',
                  }}
                >
                  <input
                    type="text"
                    placeholder={t.urlInputPlacerholder}
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
                      cursor: isDisabled() ? 'not-allowed' : 'pointer',
                    }}
                    disabled={isDisabled()}
                    onClick={checkConnection}
                  >
                    <span>{t.buttonSetText}</span>
                  </button>
                </div>
              </>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
