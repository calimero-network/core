import {
  ApiResponse,
  getAuthConfig,
  rpcClient,
  RpcError,
  RpcQueryParams,
} from '@calimero-network/calimero-client';

import {
  BlobApi,
  BlobMethod,
  BlobUploadResponse,
  BlobMetadataResponse,
  RegisterBlobRequest,
  GetBlobIdRequest,
  GetBlobMetadataRequest,
  BlobMetadata,
  ListBlobsResponse,
  CreateBlobRequest,
  CreateBlobResponse,
  ReadBlobRequest,
  ReadBlobResponse,
  TestBasicOperationsResponse,
  TestRestApiWorkflowResponse,
  TestMultipartBlobRequest,
  TestMultipartBlobResponse,
  GetStatsResponse,
} from '../blobApi';

const RequestHeaders = {
  headers: {
    'Content-Type': 'application/json',
  },
  timeout: 10000,
};

function getErrorMessage(error: RpcError): string {
  return error?.error?.cause?.info?.message || 
         error?.error?.cause?.name || 
         'An unexpected error occurred';
}

export class BlobApiDataSource implements BlobApi {
  // REST API Methods
  
  async uploadBlob(file: File, expectedHash?: string): Promise<ApiResponse<BlobUploadResponse>> {
    try {
      const config = getAuthConfig();
      if (!config || !config.appEndpointKey) {
        return {
          data: null,
          error: {
            code: 500,
            message: 'Node URL not configured',
          },
        };
      }

      // Build URL with optional hash parameter
      let url = `${config.appEndpointKey}/admin-api/blobs/upload`;
      if (expectedHash) {
        url += `?hash=${encodeURIComponent(expectedHash)}`;
      }

      const response = await fetch(url, {
        method: 'POST',
        body: file,
        headers: {
          // Let browser set Content-Type for raw binary data
        },
      });

      console.log('response', response);
      console.log('response.ok:', response.ok);
      console.log('response.bodyUsed before reading:', response.bodyUsed);

      if (!response.ok) {
        const errorText = await response.text();
        return {
          data: null,
          error: {
            code: response.status,
            message: errorText || `HTTP ${response.status}`,
          },
        };
      }

      console.log('response.bodyUsed after ok check:', response.bodyUsed);
      
      try {
        const responseText = await response.text();
        console.log('Raw response text:', responseText);
        
        const data = JSON.parse(responseText) as BlobUploadResponse;
        console.log('Parsed JSON data:', data);
        
        return {
          data,
          error: null,
        };
      } catch (jsonError) {
        console.error('JSON parsing error:', jsonError);
        return {
          data: null,
          error: {
            code: 500,
            message: `Failed to parse JSON response: ${jsonError}`,
          },
        };
      }
    } catch (error) {
      console.error('uploadBlob failed:', error);
      return {
        error: {
          code: 500,
          message: error instanceof Error ? error.message : 'An unexpected error occurred',
        },
      };
    }
  }

  async downloadBlob(blobId: string): Promise<Blob> {
    const config = getAuthConfig();
    if (!config || !config.appEndpointKey) {
      throw new Error('Node URL not configured');
    }

    const url = `${config.appEndpointKey}/admin-api/blobs/${encodeURIComponent(blobId)}`;
    const response = await fetch(url);

    if (!response.ok) {
      throw new Error(`HTTP ${response.status}: ${await response.text()}`);
    }

    return response.blob();
  }

  async getBlobMetadata(blobId: string): Promise<ApiResponse<BlobMetadataResponse>> {
    try {
      const config = getAuthConfig();
      if (!config || !config.appEndpointKey) {
        return {
          data: null,
          error: {
            code: 500,
            message: 'Node URL not configured',
          },
        };
      }

      const url = `${config.appEndpointKey}/admin-api/blobs/${encodeURIComponent(blobId)}/info`;
      const response = await fetch(url);

      if (!response.ok) {
        const errorText = await response.text();
        return {
          data: null,
          error: {
            code: response.status,
            message: errorText || `HTTP ${response.status}`,
          },
        };
      }

      const data = await response.json() as BlobMetadataResponse;
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

  // JSON RPC Methods for Blob Management

  async registerBlob(request: RegisterBlobRequest): ApiResponse<void> {
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

      const params: RpcQueryParams<RegisterBlobRequest> = {
        contextId: config.contextId,
        method: BlobMethod.REGISTER_BLOB,
        argsJson: request,
        executorPublicKey: config.executorPublicKey,
      };

      const response = await rpcClient.execute<RegisterBlobRequest, void>(
        params, 
        RequestHeaders
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
      console.error('registerBlob failed:', error);
      return {
        error: {
          code: 500,
          message: error instanceof Error ? error.message : 'An unexpected error occurred',
        },
      };
    }
  }

  async getBlobId(request: GetBlobIdRequest): ApiResponse<string> {
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

      const params: RpcQueryParams<GetBlobIdRequest> = {
        contextId: config.contextId,
        method: BlobMethod.GET_BLOB_ID,
        argsJson: request,
        executorPublicKey: config.executorPublicKey,
      };

      const response = await rpcClient.execute<GetBlobIdRequest, string>(
        params, 
        RequestHeaders
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
      console.error('getBlobId failed:', error);
      return {
        error: {
          code: 500,
          message: error instanceof Error ? error.message : 'An unexpected error occurred',
        },
      };
    }
  }

  async getBlobMetadataByName(request: GetBlobMetadataRequest): ApiResponse<BlobMetadata> {
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

      const params: RpcQueryParams<GetBlobMetadataRequest> = {
        contextId: config.contextId,
        method: BlobMethod.GET_BLOB_METADATA,
        argsJson: request,
        executorPublicKey: config.executorPublicKey,
      };

      const response = await rpcClient.execute<GetBlobMetadataRequest, BlobMetadata>(
        params, 
        RequestHeaders
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
        data: response.result.output as BlobMetadata,
        error: null,
      };
    } catch (error) {
      console.error('getBlobMetadataByName failed:', error);
      return {
        error: {
          code: 500,
          message: error instanceof Error ? error.message : 'An unexpected error occurred',
        },
      };
    }
  }

  async listBlobs(): ApiResponse<ListBlobsResponse> {
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
        method: BlobMethod.LIST_BLOBS,
        argsJson: {},
        executorPublicKey: config.executorPublicKey,
      };

      const response = await rpcClient.execute<{}, ListBlobsResponse>(
        params, 
        RequestHeaders
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
        data: response.result.output as ListBlobsResponse,
        error: null,
      };
    } catch (error) {
      console.error('listBlobs failed:', error);
      return {
        error: {
          code: 500,
          message: error instanceof Error ? error.message : 'An unexpected error occurred',
        },
      };
    }
  }

  async unregisterBlob(name: string): ApiResponse<void> {
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

      const params: RpcQueryParams<{ name: string }> = {
        contextId: config.contextId,
        method: BlobMethod.UNREGISTER_BLOB,
        argsJson: { name },
        executorPublicKey: config.executorPublicKey,
      };

      const response = await rpcClient.execute<{ name: string }, void>(
        params, 
        RequestHeaders
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
      console.error('unregisterBlob failed:', error);
      return {
        error: {
          code: 500,
          message: error instanceof Error ? error.message : 'An unexpected error occurred',
        },
      };
    }
  }

  // Legacy JSON RPC Methods (backward compatibility)

  async createBlob(request: CreateBlobRequest): ApiResponse<CreateBlobResponse> {
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

      const params: RpcQueryParams<CreateBlobRequest> = {
        contextId: config.contextId,
        method: BlobMethod.CREATE_BLOB,
        argsJson: request,
        executorPublicKey: config.executorPublicKey,
      };

      const response = await rpcClient.execute<CreateBlobRequest, CreateBlobResponse>(
        params, 
        RequestHeaders
      );

      if (response?.error) {
        return {
          error: {
            code: response.error.code ?? 500,
            message: getErrorMessage(response.error)
          }
        };
      }

      if (!response?.result?.output) {
        console.error('Invalid response format:', response);
        return {
          error: { message: 'Invalid response format', code: 500 },
          data: null,
        };
      }

      return {
        data: response.result.output as CreateBlobResponse,
        error: null,
      };
    } catch (error) {
      console.error('createBlob failed:', error);
      return {
        error: {
          code: 500,
          message: error instanceof Error ? error.message : 'An unexpected error occurred',
        },
      };
    }
  }

  async readBlob(request: ReadBlobRequest): ApiResponse<ReadBlobResponse> {
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

      const params: RpcQueryParams<ReadBlobRequest> = {
        contextId: config.contextId,
        method: BlobMethod.READ_BLOB,
        argsJson: request,
        executorPublicKey: config.executorPublicKey,
      };

      const response = await rpcClient.execute<ReadBlobRequest, ReadBlobResponse>(
        params, 
        RequestHeaders
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
        data: response.result.output as ReadBlobResponse,
        error: null,
      };
    } catch (error) {
      console.error('readBlob failed:', error);
      return {
        error: {
          code: 500,
          message: error instanceof Error ? error.message : 'An unexpected error occurred',
        },
      };
    }
  }

  // Test Methods

  async testBasicOperations(): ApiResponse<TestBasicOperationsResponse> {
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
        method: BlobMethod.TEST_BASIC_OPERATIONS,
        argsJson: {},
        executorPublicKey: config.executorPublicKey,
      };

      const response = await rpcClient.execute<{}, TestBasicOperationsResponse>(
        params, 
        RequestHeaders
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
        data: response.result.output as TestBasicOperationsResponse,
        error: null,
      };
    } catch (error) {
      console.error('testBasicOperations failed:', error);
      return {
        error: {
          code: 500,
          message: error instanceof Error ? error.message : 'An unexpected error occurred',
        },
      };
    }
  }

  async testRestApiWorkflow(): ApiResponse<TestRestApiWorkflowResponse> {
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
        method: BlobMethod.TEST_REST_API_WORKFLOW,
        argsJson: {},
        executorPublicKey: config.executorPublicKey,
      };

      const response = await rpcClient.execute<{}, TestRestApiWorkflowResponse>(
        params, 
        RequestHeaders
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
        data: response.result.output as TestRestApiWorkflowResponse,
        error: null,
      };
    } catch (error) {
      console.error('testRestApiWorkflow failed:', error);
      return {
        error: {
          code: 500,
          message: error instanceof Error ? error.message : 'An unexpected error occurred',
        },
      };
    }
  }

  async testMultipartBlob(request: TestMultipartBlobRequest): ApiResponse<TestMultipartBlobResponse> {
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

      const params: RpcQueryParams<TestMultipartBlobRequest> = {
        contextId: config.contextId,
        method: BlobMethod.TEST_MULTIPART_BLOB,
        argsJson: request,
        executorPublicKey: config.executorPublicKey,
      };

      const response = await rpcClient.execute<TestMultipartBlobRequest, TestMultipartBlobResponse>(
        params, 
        RequestHeaders
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
        data: response.result.output as TestMultipartBlobResponse,
        error: null,
      };
    } catch (error) {
      console.error('testMultipartBlob failed:', error);
      return {
        error: {
          code: 500,
          message: error instanceof Error ? error.message : 'An unexpected error occurred',
        },
      };
    }
  }

  async getStats(): ApiResponse<GetStatsResponse> {
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
        method: BlobMethod.GET_STATS,
        argsJson: {},
        executorPublicKey: config.executorPublicKey,
      };

      const response = await rpcClient.execute<{}, GetStatsResponse>(
        params, 
        RequestHeaders
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
        data: response.result.output as GetStatsResponse,
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
} 