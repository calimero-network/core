import { useCallback, useEffect, useState } from 'react';
import React from 'react';
import apiClient from '../../api';
import translations from '../../constants/en.global.json';
import { styled } from 'styled-components';
<<<<<<< HEAD
import Loading from '../common/Loading';
=======
>>>>>>> 2e17bab7 (feat:(1) admin remove sdk components 3)

const Wrapper = styled.div`
    display: flex;
    height: 100vh;
    justify-content: center;
    background-color: #111111;

    .flex-wrapper {
        display: flex;
        flex-direction: column;
        justify-content: center;
        align-items: center;

        .inner-wrapper {
            display: flex;
            flex-direction: column;
            align-items: center;
            background-color: #1c1c1c;
            padding: 2rem;
            gap: 1rem;
            border-radius: 0.5rem;

            .content-wrapper {
                display: flex;
                flex-direction: column;
                justify-content: center;
                align-items: center;
                gap: 2rem;
                padding: 0 3.5rem;

                .title {
                    color: white;
                    font-size: 2.5rem;
                    font-weight: 600;
                }

                .popup-wrapper {
                    display: flex;
                    flex-direction: column;
                    gap: 0.5rem;

                    .input-field {
                        width: 400px;
                        padding: 0.5rem;
                        border-radius: 0.375rem;
                    }

                    .error {
                        color: #ef4444;
                        font-size: 0.875;
                    }

                    .button {
                        background-color: #6b7280;
                        color: white;
                        width: 100%;
                        display: flex;
                        justify-content: center;
                        align-items: center;
                        padding: 0.5rem;
                        border-radius: 0.375rem;
                        border: none;
                        outline: none;
                        gap: 0.5rem;
                        height: 46px;
                        font-size: 1rem;
                        font-weight: 500;
                    }
                }
        }
    }
`;

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
  }, [getNodeUrl]);

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
    <Wrapper>
      <div className="flex-wrapper">
        <div className="inner-wrapper">
          <div className="content-wrapper">
            <div className="title">{t.modalTitle}</div>
            {loading ? (
              <Loading
                loaderColor={'#FF7A00'}
                loaderSize={'48px'}
                borderSize={'5px'}
              />
            ) : (
              <>
                <div className="popup-wrapper">
                  <input
                    type="text"
                    placeholder={t.urlInputPlacerholder}
                    inputMode="url"
                    value={url?.toString() || ''}
                    onChange={(e: { target: { value: string } }) => {
                      handleChange(e.target.value);
                    }}
                    className="input-field"
                  />
                  <div className="error">{error}</div>
                  <button
                    className="button"
                    style={{
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
    </Wrapper>
  );
}
