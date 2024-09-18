import apiClient from '../api';
import { JwtTokenResponse } from '../api/nodeApi';
import { setAccessToken, setRefreshToken } from '../storage';
import { ResponseData } from '../types';

interface GetNewJwtTokenProps {
  refreshToken: string;
  getNodeUrl: () => string;
}

export const getNewJwtToken = async ({
  refreshToken,
  getNodeUrl,
}: GetNewJwtTokenProps) => {
  const tokenResponse: ResponseData<JwtTokenResponse> = await apiClient
    .node()
    .refreshToken(refreshToken, getNodeUrl());
  if (tokenResponse.data) {
    setAccessToken(tokenResponse.data.access_token);
    setRefreshToken(tokenResponse.data.refresh_token);
  }
};
