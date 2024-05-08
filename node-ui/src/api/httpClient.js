import { AxiosError } from "axios";

export class AxiosHttpClient {
  axios;

  constructor(axios) {
    this.axios = axios;
  }

  async get(url, headers) {
    return this.request(
      this.axios.get(url, {
        headers: headers?.reduce((acc, curr) => ({ ...acc, ...curr }), {}),
      })
    );
  }

  async post(url, body, headers) {
    return this.request(
      this.axios.post(url, body, {
        headers: headers?.reduce((acc, curr) => ({ ...acc, ...curr }), {}),
      })
    );
  }

  async delete(url, headers) {
    return this.request(this.axios.delete(url, {
        headers: headers?.reduce((acc, curr) => ({...acc, ...curr}), {})
    }));
}

  async request(promise) {
    try {
      const response = await promise;

      if (response.config.method.toUpperCase() === "HEAD") {
        return {
          data: undefined,
        };
      } else {
        return response.data;
      }
    } catch (e) {
      if (e instanceof AxiosError) {
        if (e.config.method.toUpperCase() === "HEAD") {
          return {
            error: {
              code: e.request.status,
              message: e.message,
            },
          };
        }

        const error = e.response?.data.error;
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

const GENERIC_ERROR = {
  code: 500,
  message: "Something went wrong",
};
