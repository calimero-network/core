import { Axios, AxiosError, AxiosResponse } from "axios";
import { ErrorResponse, ResponseData } from "../api-response";

export interface Header {
  [key: string]: string;
}

export interface HttpClient {
  get<T>(url: string, headers?: Header[]): Promise<ResponseData<T>>;
  post<T>(
    url: string,
    body?: unknown,
    headers?: Header[]
  ): Promise<ResponseData<T>>;
  put<T>(
    url: string,
    body?: unknown,
    headers?: Header[]
  ): Promise<ResponseData<T>>;
  delete<T>(
    url: string,
    body?: unknown,
    headers?: Header[]
  ): Promise<ResponseData<T>>;
  patch<T>(
    url: string,
    body?: unknown,
    headers?: Header[]
  ): Promise<ResponseData<T>>;
  head(url: string, headers?: Header[]): Promise<ResponseData<void>>;
}

export class AxiosHttpClient implements HttpClient {
  private axios: Axios;

  constructor(axios: Axios) {
    this.axios = axios;
  }

  async get<T>(url: string, headers?: Header[]): Promise<ResponseData<T>> {
    return this.request<T>(
      this.axios.get<ResponseData<T>>(url, {
        headers: headers?.reduce((acc, curr) => ({ ...acc, ...curr }), {}),
      })
    );
  }

  async post<T>(
    url: string,
    body?: unknown,
    headers?: Header[]
  ): Promise<ResponseData<T>> {
    return this.request<T>(
      this.axios.post<ResponseData<T>>(url, body, {
        headers: headers?.reduce((acc, curr) => ({ ...acc, ...curr }), {}),
      })
    );
  }

  async put<T>(
    url: string,
    body?: unknown,
    headers?: Header[]
  ): Promise<ResponseData<T>> {
    return this.request<T>(
      this.axios.put<ResponseData<T>>(url, body, {
        headers: headers?.reduce((acc, curr) => ({ ...acc, ...curr }), {}),
      })
    );
  }

  async delete<T>(url: string, headers?: Header[]): Promise<ResponseData<T>> {
    return this.request<T>(
      this.axios.delete<ResponseData<T>>(url, {
        headers: headers?.reduce((acc, curr) => ({ ...acc, ...curr }), {}),
      })
    );
  }

  async patch<T>(
    url: string,
    body?: unknown,
    headers?: Header[]
  ): Promise<ResponseData<T>> {
    return this.request<T>(
      this.axios.patch<ResponseData<T>>(url, body, {
        headers: headers?.reduce((acc, curr) => ({ ...acc, ...curr }), {}),
      })
    );
  }

  async head(url: string, headers?: Header[]): Promise<ResponseData<void>> {
    return this.request(
      this.axios.head(url, {
        headers: headers?.reduce((acc, curr) => ({ ...acc, ...curr }), {}),
      })
    );
  }

  private async request<T>(
    promise: Promise<AxiosResponse<ResponseData<T>>>
  ): Promise<ResponseData<T>> {
    try {
      const response = await promise;

      //head does not return body so we are adding data manually
      if (response?.config?.method?.toUpperCase() === "HEAD") {
        return {
          data: null as T,
        };
      } else {
        return response.data;
      }
    } catch (e: unknown) {
      if (e instanceof AxiosError) {
        //head does not return body so we are adding error manually
        if (e?.config?.method?.toUpperCase() === "HEAD") {
          return {
            error: {
              code: e.request.status,
              message: e.message,
            },
          };
        }

        const error: ErrorResponse = e.response?.data.error;
        //TODO make code mandatory
        if (!error || !error.message) {
          return {
            error: GENERIC_ERROR,
          };
        }
        return {
          error: {
            code: error.code,
            message: error.message,
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
  message: "Something went wrong",
};
