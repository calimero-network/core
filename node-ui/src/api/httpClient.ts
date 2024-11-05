import { Axios, AxiosError, AxiosResponse } from 'axios';
import { ErrorResponse, ResponseData } from './response';

export interface Header {
  [key: string]: string;
}

export interface HttpClient {
  get<T>(url: string, headers?: Header): Promise<ResponseData<T>>;
  post<T>(
    url: string,
    body?: unknown,
    headers?: Header,
  ): Promise<ResponseData<T>>;
  put<T>(
    url: string,
    body?: unknown,
    headers?: Header,
  ): Promise<ResponseData<T>>;
  delete<T>(
    url: string,
    body?: unknown,
    headers?: Header,
  ): Promise<ResponseData<T>>;
  patch<T>(
    url: string,
    body?: unknown,
    headers?: Header,
  ): Promise<ResponseData<T>>;
  head(url: string, headers?: Header): Promise<ResponseData<void>>;
}
// AxiosHttpClient.ts
export class AxiosHttpClient implements HttpClient {
  private axios: Axios;
  private showServerDownPopup: () => void;

  constructor(axios: Axios, showServerDownPopup: () => void) {
    this.axios = axios;
    this.showServerDownPopup = showServerDownPopup;

    this.axios.interceptors.response.use(
      (response: AxiosResponse) => response,
      (error: AxiosError) => {
        if (error.response?.status === 401) {
          window.location.href = '/admin-dashboard/';
        }
        if (!error.response) {
          this.showServerDownPopup();
        }
        return Promise.reject(error);
      },
    );
  }

  async get<T>(url: string, headers: Header = {}): Promise<ResponseData<T>> {
    return this.request<T>(this.axios.get<ResponseData<T>>(url, { headers }));
  }

  async post<T>(
    url: string,
    body?: unknown,
    headers: Header = {},
  ): Promise<ResponseData<T>> {
    return this.request<T>(
      this.axios.post<ResponseData<T>>(url, body, { headers }),
    );
  }

  async put<T>(
    url: string,
    body?: unknown,
    headers: Header = {},
  ): Promise<ResponseData<T>> {
    return this.request<T>(
      this.axios.put<ResponseData<T>>(url, body, { headers }),
    );
  }

  async delete<T>(url: string, headers: Header = {}): Promise<ResponseData<T>> {
    return this.request<T>(
      this.axios.delete<ResponseData<T>>(url, { headers }),
    );
  }

  async patch<T>(
    url: string,
    body?: unknown,
    headers: Header = {},
  ): Promise<ResponseData<T>> {
    return this.request<T>(
      this.axios.patch<ResponseData<T>>(url, body, { headers }),
    );
  }

  async head(url: string, headers: Header = {}): Promise<ResponseData<void>> {
    return this.request(this.axios.head(url, { headers }));
  }

  private async request<T>(
    promise: Promise<AxiosResponse<ResponseData<T>>>,
  ): Promise<ResponseData<T>> {
    try {
      const response = await promise;

      //head does not return body so we are adding data manually
      // @ts-ignore
      if (response.config.method.toUpperCase() === 'HEAD') {
        return {
          data: undefined as unknown as T,
        };
      } else {
        return response.data;
      }
    } catch (e: unknown) {
      if (e instanceof AxiosError) {
        //head does not return body so we are adding error manually
        if (e.config?.method?.toUpperCase() === 'HEAD') {
          return {
            error: {
              code: e.request.status,
              message: e.message,
            },
          };
        }

        const errorResponse = e.response?.data as ResponseData<T>;
        const error: ErrorResponse | null | undefined = errorResponse.error;
        if (!errorResponse && (!error || !error.message || !error.code)) {
          return {
            error: GENERIC_ERROR,
          };
        }
        if (typeof errorResponse === 'string') {
          return {
            error: {
              code: e.request.status,
              message: errorResponse,
            },
          };
        }
        if (typeof error === 'string') {
          return {
            error: {
              code: e.request.status,
              message: error,
            },
          };
        }
        return {
          error: {
            code: error?.code || e.request.status,
            message: error?.message!,
          },
        };
      }
      return {
        error: GENERIC_ERROR,
      };
    }
  }
}

const GENERIC_ERROR: ErrorResponse = {
  code: 500,
  message: 'Something went wrong',
};
