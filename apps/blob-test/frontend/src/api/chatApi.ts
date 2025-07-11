import { ApiResponse, RpcQueryParams, rpcClient, getAuthConfig } from '@calimero-network/calimero-client';

// Chat API interfaces for JSON RPC functionality
export interface Message {
  id: number;
  sender: string;
  text: string;
  attachments: Attachment[];
  timestamp: number;
}

export interface Attachment {
  original_name: string;
  original_blob_id: string;
  original_size: number;
  compressed_blob_id: string;
  compressed_size: number;
  content_type?: string;
  compression_ratio: number;
}

export interface SendMessageRequest {
  sender: string;
  text: string;
  attachment_blob_ids: string[];
  attachment_names: string[];
  attachment_sizes: number[];
  attachment_content_types: (string | null)[];
}

export interface ChatStats extends Record<string, number> {
  total_messages: number;
  total_attachments: number;
  total_original_size_bytes: number;
  total_compressed_size_bytes: number;
  compression_savings_percent: number;
}

// Method names that match the Rust backend  
export enum ChatMethod {
  SEND_MESSAGE = 'send_message',
  GET_MESSAGES = 'get_messages',
  GET_MESSAGE = 'get_message',
  GET_DECOMPRESSED_BLOB_ID = 'get_decompressed_blob_id',
  GET_STATS = 'get_stats',
  CLEAR_MESSAGES = 'clear_messages',
}

// File upload interface for the UI
export interface FileUpload {
  file: File;
  blob_id?: string;
  uploading: boolean;
  uploaded: boolean;
  progress: number;
  error?: string;
}

const RequestConfig = { timeout: 30000 };

function getErrorMessage(error: any): string {
  if (typeof error === 'string') return error;
  if (error?.message) return error.message;
  if (error?.data) return JSON.stringify(error.data);
  return 'An unexpected error occurred';
}

// Chat API implementation using JSON RPC
export class ChatApi {
  
  async sendMessage(request: SendMessageRequest): ApiResponse<number> {
    try {
      const config = getAuthConfig();

      if (!config || !config.contextId || !config.executorPublicKey) {
        return {
          data: null,
          error: {
            code: 500,
            message: 'Authentication configuration not found',
          },
        };
      }

      const params: RpcQueryParams<SendMessageRequest> = {
        contextId: config.contextId,
        method: ChatMethod.SEND_MESSAGE,
        argsJson: request,
        executorPublicKey: config.executorPublicKey,
      };

      const response = await rpcClient.execute<SendMessageRequest, number>(
        params, 
        RequestConfig
      );

      if (response?.error) {
        return {
          error: {
            code: response.error.code ?? 500,
            message: getErrorMessage(response.error)
          }
        };
      }

      return {
        data: response.result.output as number,
        error: null,
      };
    } catch (error) {
      console.error('sendMessage failed:', error);
      return {
        error: {
          code: 500,
          message: error instanceof Error ? error.message : 'An unexpected error occurred',
        },
      };
    }
  }

  async getMessages(): ApiResponse<Message[]> {
    try {
      const config = getAuthConfig();

      if (!config || !config.contextId || !config.executorPublicKey) {
        return {
          data: null,
          error: {
            code: 500,
            message: 'Authentication configuration not found',
          },
        };
      }

      const params: RpcQueryParams<{}> = {
        contextId: config.contextId,
        method: ChatMethod.GET_MESSAGES,
        argsJson: {},
        executorPublicKey: config.executorPublicKey,
      };

      const response = await rpcClient.execute<{}, Message[]>(
        params, 
        RequestConfig
      );

      if (response?.error) {
        return {
          error: {
            code: response.error.code ?? 500,
            message: getErrorMessage(response.error)
          }
        };
      }

      return {
        data: response.result.output as Message[],
        error: null,
      };
    } catch (error) {
      console.error('getMessages failed:', error);
      return {
        error: {
          code: 500,
          message: error instanceof Error ? error.message : 'An unexpected error occurred',
        },
      };
    }
  }

  async getMessage(messageId: number): ApiResponse<Message> {
    try {
      const config = getAuthConfig();

      if (!config || !config.contextId || !config.executorPublicKey) {
        return {
          data: null,
          error: {
            code: 500,
            message: 'Authentication configuration not found',
          },
        };
      }

      const params: RpcQueryParams<{ message_id: number }> = {
        contextId: config.contextId,
        method: ChatMethod.GET_MESSAGE,
        argsJson: { message_id: messageId },
        executorPublicKey: config.executorPublicKey,
      };

      const response = await rpcClient.execute<{ message_id: number }, Message>(
        params, 
        RequestConfig
      );

      if (response?.error) {
        return {
          error: {
            code: response.error.code ?? 500,
            message: getErrorMessage(response.error)
          }
        };
      }

      return {
        data: response.result.output as Message,
        error: null,
      };
    } catch (error) {
      console.error('getMessage failed:', error);
      return {
        error: {
          code: 500,
          message: error instanceof Error ? error.message : 'An unexpected error occurred',
        },
      };
    }
  }

  async getDecompressedBlobId(compressedBlobId: string): ApiResponse<string> {
    try {
      const config = getAuthConfig();

      if (!config || !config.contextId || !config.executorPublicKey) {
        return {
          data: null,
          error: {
            code: 500,
            message: 'Authentication configuration not found',
          },
        };
      }

      const params: RpcQueryParams<{ compressed_blob_id_str: string }> = {
        contextId: config.contextId,
        method: ChatMethod.GET_DECOMPRESSED_BLOB_ID,
        argsJson: { compressed_blob_id_str: compressedBlobId },
        executorPublicKey: config.executorPublicKey,
      };

      const response = await rpcClient.execute<{ compressed_blob_id_str: string }, string>(
        params, 
        RequestConfig
      );

      if (response?.error) {
        return {
          error: {
            code: response.error.code ?? 500,
            message: getErrorMessage(response.error)
          }
        };
      }

      return {
        data: response.result.output as string,
        error: null,
      };
    } catch (error) {
      console.error('getDecompressedBlobId failed:', error);
      return {
        error: {
          code: 500,
          message: error instanceof Error ? error.message : 'An unexpected error occurred',
        },
      };
    }
  }

  async getStats(): ApiResponse<ChatStats> {
    try {
      const config = getAuthConfig();

      if (!config || !config.contextId || !config.executorPublicKey) {
        return {
          data: null,
          error: {
            code: 500,
            message: 'Authentication configuration not found',
          },
        };
      }

      const params: RpcQueryParams<{}> = {
        contextId: config.contextId,
        method: ChatMethod.GET_STATS,
        argsJson: {},
        executorPublicKey: config.executorPublicKey,
      };

      const response = await rpcClient.execute<{}, ChatStats>(
        params, 
        RequestConfig
      );

      if (response?.error) {
        return {
          error: {
            code: response.error.code ?? 500,
            message: getErrorMessage(response.error)
          }
        };
      }

      return {
        data: response.result.output as ChatStats,
        error: null,
      };
    } catch (error) {
      console.error('getStats failed:', error);
      return {
        error: {
          code: 500,
          message: error instanceof Error ? error.message : 'An unexpected error occurred',
        },
      };
    }
  }

  async clearMessages(): ApiResponse<void> {
    try {
      const config = getAuthConfig();

      if (!config || !config.contextId || !config.executorPublicKey) {
        return {
          data: null,
          error: {
            code: 500,
            message: 'Authentication configuration not found',
          },
        };
      }

      const params: RpcQueryParams<{}> = {
        contextId: config.contextId,
        method: ChatMethod.CLEAR_MESSAGES,
        argsJson: {},
        executorPublicKey: config.executorPublicKey,
      };

      const response = await rpcClient.execute<{}, void>(
        params, 
        RequestConfig
      );

      if (response?.error) {
        return {
          error: {
            code: response.error.code ?? 500,
            message: getErrorMessage(response.error)
          }
        };
      }

      return {
        data: undefined,
        error: null,
      };
    } catch (error) {
      console.error('clearMessages failed:', error);
      return {
        error: {
          code: 500,
          message: error instanceof Error ? error.message : 'An unexpected error occurred',
        },
      };
    }
  }
}
