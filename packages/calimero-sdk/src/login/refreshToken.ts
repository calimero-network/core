import apiClient from '../api';
import { JwtTokenResponse } from '../api/nodeApi';
import {
  clearJWT,
  getRefreshToken,
  setAccessToken,
  setRefreshToken,
} from '../storage';
import { ErrorResponse, ResponseData, RpcError } from '../types';

interface GetNewJwtTokenProps {
  refreshToken: string;
  getNodeUrl: () => string;
}

type JsonRpcErrorType =
    'UnknownServerError' |
    'RpcExecutionError' |
    'FunctionCallError' |
    'CallError' |
    'MissmatchedRequestIdError' |
    'InvalidRequestError';

const errorTypes: JsonRpcErrorType[] = [
  'UnknownServerError',
  'RpcExecutionError',
  'FunctionCallError',
  'CallError',
  'MissmatchedRequestIdError',
  'InvalidRequestError'
];

export const getNewJwtToken = async ({
  refreshToken,
  getNodeUrl,
}: GetNewJwtTokenProps): Promise<ResponseData<JwtTokenResponse>> => {
  const tokenResponse: ResponseData<JwtTokenResponse> = await apiClient
    .node()
    .refreshToken(refreshToken, getNodeUrl());

  if (tokenResponse.error) {
    return { error: tokenResponse.error };
  }
  setAccessToken(tokenResponse.data.access_token);
  setRefreshToken(tokenResponse.data.refresh_token);
  return { data: tokenResponse.data };
};

export const handleRpcError = async (
  error: RpcError,
  getNodeUrl: () => string,
): Promise<ErrorResponse> => {
  const invalidSession = {
    message: 'Your session is no longer valid. Please log in again.',
    code: 401,
  };
  const expiredSession = {
    message: '',
    code: 403,
  };
  const unknownMessage = {
    message: 'Server Error: Something went wrong. Please try again.',
    code: 500,
  };

  if (error.code === 401) {
    if (error?.error?.cause?.info?.message === 'Token expired.') {
      try {
        const refreshToken = getRefreshToken();
        const response = await getNewJwtToken({ refreshToken, getNodeUrl });
        if (response?.error) {
          clearJWT();
          return invalidSession;
        }
        return expiredSession;
      } catch (error) {
        clearJWT();
        return invalidSession;
      }
    }
    clearJWT();
    return invalidSession;
  }
  const errorType = error?.error?.name;
  if (errorTypes.includes(errorType as JsonRpcErrorType)) {
    return {
      message: `${errorType}: ${error.error.cause.info.message}`,
      code: error.code,
    };
  } else {
    return unknownMessage;
  }
};
