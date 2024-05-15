export type ResponseData<D> =
  | {
      data: D;
      error?: null;
    }
  | {
      data?: null;
      error: ErrorResponse;
    };

export type ErrorResponse = {
  code?: number;
  message: string;
};

export interface SuccessResponse {
  success: boolean;
}

export type ApiResponse<T> = Promise<ResponseData<T>>;
