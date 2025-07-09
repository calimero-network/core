import { 
  ApiResponse, 
  RpcQueryParams, 
  rpcClient
} from '@calimero-network/calimero-client';
import { 
  ChatApi, 
  BlobUploadResponse, 
  BlobMetadataResponse,
  SendMessageRequest,
  Message,
  Attachment,
  ChatStats,
  ChatMethod
} from '../blobApi';
import { getAuthConfig } from '@calimero-network/calimero-client';

const BASE_URL = process.env.REACT_APP_NODE_URL || 'http://localhost:2428';
const RequestConfig = { timeout: 30000 };

function getErrorMessage(error: any): string {
  if (typeof error === 'string') return error;
  if (error?.message) return error.message;
  if (error?.data) return JSON.stringify(error.data);
  return 'An unexpected error occurred';
}

export class ChatApiDataSource implements ChatApi {
  
  // REST API Methods for file upload/download

  async uploadBlob(
    file: File, 
    onProgress?: (progress: number) => void, 
    expectedHash?: string
  ): Promise<ApiResponse<BlobUploadResponse>> {
    return this.uploadBlobRaw(file, onProgress, expectedHash);
  }

  async uploadBlobRaw(
    file: File, 
    onProgress?: (progress: number) => void, 
    expectedHash?: string
  ): Promise<ApiResponse<BlobUploadResponse>> {
    // Read file as ArrayBuffer for raw binary upload
    const fileArrayBuffer = await file.arrayBuffer();

    return new Promise((resolve) => {
      const xhr = new XMLHttpRequest();

      xhr.upload.addEventListener('progress', (event) => {
        if (event.lengthComputable && onProgress) {
          const progress = (event.loaded / event.total) * 100;
          onProgress(progress);
        }
      });

      xhr.addEventListener('load', () => {
        try {
          if (xhr.status === 200) {
            const response = JSON.parse(xhr.responseText);
            resolve({
              data: response,
              error: null,
            });
          } else {
            const errorResponse = JSON.parse(xhr.responseText);
            resolve({
              data: null,
              error: {
                code: xhr.status,
                message: errorResponse.error || `HTTP ${xhr.status}: ${xhr.statusText}`,
              },
            });
          }
        } catch (error) {
          resolve({
            data: null,
            error: {
              code: xhr.status || 500,
              message: error instanceof Error ? error.message : 'Failed to parse response',
            },
          });
        }
      });

      xhr.addEventListener('error', () => {
        resolve({
          data: null,
          error: {
            code: 500,
            message: 'Network error occurred during upload',
          },
        });
      });

      // Build URL with query parameters
      let url = `${BASE_URL}/admin-api/blobs/upload-raw`;
      if (expectedHash) {
        url += `?hash=${encodeURIComponent(expectedHash)}`;
      }

      xhr.open('POST', url);
      xhr.setRequestHeader('Content-Type', 'application/octet-stream');
      xhr.send(fileArrayBuffer);
    });
  }

  async downloadBlob(blobId: string): Promise<Blob> {
    const response = await fetch(`${BASE_URL}/admin-api/blobs/${blobId}`);
    
    if (!response.ok) {
      throw new Error(`Failed to download blob: ${response.status} ${response.statusText}`);
    }
    
    return response.blob();
  }

  async getBlobMetadata(blobId: string): Promise<ApiResponse<BlobMetadataResponse>> {
    try {
      const response = await fetch(`${BASE_URL}/admin-api/blobs/${blobId}/info`);
      
      if (!response.ok) {
        return {
          data: null,
          error: {
            code: response.status,
            message: `HTTP ${response.status}: ${response.statusText}`,
          },
        };
      }
      
      const data = await response.json();
      return {
        data,
        error: null,
      };
    } catch (error) {
      console.error('getBlobMetadata failed:', error);
      return {
        error: {
          code: 500,
          message: error instanceof Error ? error.message : 'An unexpected error occurred',
        },
      };
    }
  }

  // Helper methods for attachment handling

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

  // JSON RPC Methods for Chat Functionality

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