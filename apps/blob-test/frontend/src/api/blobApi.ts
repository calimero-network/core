import { ApiResponse } from "@calimero-network/calimero-client";

// REST API interfaces for direct blob upload/download
export interface BlobUploadResponse {
  blob_id: string;
  size: number;
}

export interface BlobMetadataResponse {
  blob_id: string;
  size: number;
  exists: boolean;
}

// JSON RPC interfaces for blob registration and management
export interface RegisterBlobRequest {
  name: string;
  blob_id: string;
  size: number;
  content_type?: string;
}

export interface GetBlobIdRequest {
  name: string;
}

export interface GetBlobIdResponse {
  // Returns the blob_id as a string directly
}

export interface GetBlobMetadataRequest {
  name: string;
}

export interface BlobMetadata {
  blob_id: string;
  size: number;
  content_type?: string;
  uploaded_at: number;
}

export interface ListBlobsResponse extends Record<string, BlobMetadata> {
  // BTreeMap<String, BlobMetadata> returned directly as key-value pairs
}

// Legacy JSON RPC interfaces (backward compatibility)
export interface CreateBlobRequest {
  name: string;
  data: number[]; // Vec<u8> as array of numbers
}

export interface CreateBlobResponse {
  blob_id: string;
}

export interface ReadBlobRequest {
  name: string;
}

export interface ReadBlobResponse extends Array<number> {
  // Vec<u8> returned directly as array of numbers
}

export interface TestBasicOperationsResponse {
  result: string;
}

export interface TestRestApiWorkflowResponse {
  result: string;
}

export interface TestMultipartBlobRequest {
  chunks: number[][]; // Vec<Vec<u8>>
}

export interface TestMultipartBlobResponse {
  result: string;
}

export interface GetStatsResponse extends Record<string, number> {
  // BTreeMap<String, u32> returned directly as key-value pairs
}

// Method names that match the Rust backend
export enum BlobMethod {
  // New REST API workflow methods
  REGISTER_BLOB = 'register_blob',
  GET_BLOB_ID = 'get_blob_id',
  GET_BLOB_METADATA = 'get_blob_metadata',
  LIST_BLOBS = 'list_blobs',
  UNREGISTER_BLOB = 'unregister_blob',
  
  // Legacy methods (backward compatibility)
  CREATE_BLOB = 'create_blob',
  READ_BLOB = 'read_blob',
  
  // Test methods
  TEST_BASIC_OPERATIONS = 'test_basic_operations',
  TEST_REST_API_WORKFLOW = 'test_rest_api_workflow',
  TEST_MULTIPART_BLOB = 'test_multipart_blob',
  GET_STATS = 'get_stats',
}

// Main API interface
export interface BlobApi {
  // REST API methods (to be implemented in data source)
  uploadBlob(file: File, expectedHash?: string): Promise<ApiResponse<BlobUploadResponse>>;
  downloadBlob(blobId: string): Promise<Blob>;
  getBlobMetadata(blobId: string): Promise<ApiResponse<BlobMetadataResponse>>;
  
  // JSON RPC methods for blob management
  registerBlob(request: RegisterBlobRequest): ApiResponse<void>;
  getBlobId(request: GetBlobIdRequest): ApiResponse<string>;
  getBlobMetadataByName(request: GetBlobMetadataRequest): ApiResponse<BlobMetadata>;
  listBlobs(): ApiResponse<ListBlobsResponse>;
  unregisterBlob(name: string): ApiResponse<void>;
  
  // Legacy JSON RPC methods (backward compatibility)
  createBlob(request: CreateBlobRequest): ApiResponse<CreateBlobResponse>;
  readBlob(request: ReadBlobRequest): ApiResponse<ReadBlobResponse>;
  
  // Test methods
  testBasicOperations(): ApiResponse<TestBasicOperationsResponse>;
  testRestApiWorkflow(): ApiResponse<TestRestApiWorkflowResponse>;
  testMultipartBlob(request: TestMultipartBlobRequest): ApiResponse<TestMultipartBlobResponse>;
  getStats(): ApiResponse<GetStatsResponse>;
} 