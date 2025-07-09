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

// Chat API interfaces
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

// File upload interface for immediate upload
export interface FileUpload {
  file: File;
  blob_id?: string;
  uploading: boolean;
  uploaded: boolean;
  progress: number;
  error?: string;
}

// Attachment download helper
export interface AttachmentDownload {
  attachment: Attachment;
  requesting_decompressed_id: boolean;
  downloading: boolean;
  completed: boolean;
  progress: number;
  error?: string;
  decompressed_blob_id?: string; // For HTTP download
  blobUrl?: string; // For browser download/display
}

// Main API interface
export interface ChatApi {
  // REST API methods (direct HTTP upload/download)
  uploadBlob(file: File, onProgress?: (progress: number) => void, expectedHash?: string): Promise<ApiResponse<BlobUploadResponse>>;
  downloadBlob(blobId: string): Promise<Blob>;
  getBlobMetadata(blobId: string): Promise<ApiResponse<BlobMetadataResponse>>;
  
  // JSON RPC methods for chat functionality  
  sendMessage(request: SendMessageRequest): ApiResponse<number>; // Returns message_id
  getMessages(): ApiResponse<Message[]>;
  getMessage(messageId: number): ApiResponse<Message>;
  getDecompressedBlobId(compressedBlobId: string): ApiResponse<string>; // Returns decompressed blob ID
  getStats(): ApiResponse<ChatStats>;
  clearMessages(): ApiResponse<void>;
} 