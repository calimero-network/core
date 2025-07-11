import React, { useState, useEffect, useRef, useCallback, DragEvent } from 'react';
import { blobClient } from '@calimero-network/calimero-client';
import { ChatApi, Message, FileUpload, Attachment } from '../api/chatApi';

const chatApi = new ChatApi();

interface MessageWithFiles extends Message {
  isExpanded?: boolean;
}

export default function ChatPage() {
  const [messages, setMessages] = useState<MessageWithFiles[]>([]);
  const [messageText, setMessageText] = useState('');
  const [senderName, setSenderName] = useState('User');
  const [files, setFiles] = useState<FileUpload[]>([]);
  const [output, setOutput] = useState('');
  const [stats, setStats] = useState<any>({});
  const [isDragOver, setIsDragOver] = useState(false);
  const [loading, setLoading] = useState(false);
  
  const fileInputRef = useRef<HTMLInputElement>(null);
  const messagesEndRef = useRef<HTMLDivElement>(null);

  const scrollToBottom = () => {
    messagesEndRef.current?.scrollIntoView({ behavior: 'smooth' });
  };

  useEffect(() => {
    scrollToBottom();
  }, [messages]);

  const appendOutput = (message: string) => {
    setOutput(prev => prev + '\n' + new Date().toLocaleTimeString() + ': ' + message);
  };

  const loadMessages = async () => {
    try {
      const response = await chatApi.getMessages();
      if (response.error) {
        appendOutput(`Error loading messages: ${response.error.message}`);
      } else {
        setMessages(response.data || []);
        appendOutput(`Loaded ${response.data?.length || 0} messages`);
      }
    } catch (error) {
      appendOutput(`Failed to load messages: ${error}`);
    }
  };

  const loadStats = async () => {
    try {
      const response = await chatApi.getStats();
      if (response.error) {
        appendOutput(`Error loading stats: ${response.error.message}`);
      } else {
        setStats(response.data || {});
      }
    } catch (error) {
      appendOutput(`Failed to load stats: ${error}`);
    }
  };

  useEffect(() => {
    loadMessages();
    loadStats();
  }, []);

  const handleDrop = useCallback((e: DragEvent<HTMLDivElement>) => {
    e.preventDefault();
    setIsDragOver(false);
    
    const droppedFiles = Array.from(e.dataTransfer.files);
    handleFileSelection(droppedFiles);
  }, []);

  const handleDragOver = useCallback((e: DragEvent<HTMLDivElement>) => {
    e.preventDefault();
    setIsDragOver(true);
  }, []);

  const handleDragLeave = useCallback((e: DragEvent<HTMLDivElement>) => {
    e.preventDefault();
    setIsDragOver(false);
  }, []);

  const handleFileSelection = (selectedFiles: File[]) => {
    const newFiles: FileUpload[] = selectedFiles.map(file => ({
      file,
      uploading: true,
      uploaded: false,
      progress: 0,
    }));
    
    setFiles(prev => [...prev, ...newFiles]);
    
    // Start uploading each file immediately
    newFiles.forEach((fileUpload, index) => {
      uploadFile(fileUpload, files.length + index);
    });
  };

  const uploadFile = async (fileUpload: FileUpload, index: number) => {
    try {
      appendOutput(`Starting upload of ${fileUpload.file.name}...`);
      
      const response = await blobClient.uploadBlob(
        fileUpload.file,
        (progress: number) => {
          setFiles(prev => prev.map((f, i) => 
            i === index ? { ...f, progress } : f
          ));
        }
      );

      if (response.error) {
        setFiles(prev => prev.map((f, i) => 
          i === index ? { ...f, uploading: false, error: response.error!.message } : f
        ));
        appendOutput(`Upload failed for ${fileUpload.file.name}: ${response.error.message}`);
      } else {
        setFiles(prev => prev.map((f, i) => 
          i === index ? { 
            ...f, 
            uploading: false, 
            uploaded: true, 
            blob_id: response.data!.blob_id,
            progress: 100 
          } : f
        ));
        appendOutput(`Upload completed for ${fileUpload.file.name}: ${response.data!.blob_id}`);
      }
    } catch (error) {
      setFiles(prev => prev.map((f, i) => 
        i === index ? { ...f, uploading: false, error: `Upload error: ${error}` } : f
      ));
      appendOutput(`Upload error for ${fileUpload.file.name}: ${error}`);
    }
  };

  const removeFile = (index: number) => {
    setFiles(prev => prev.filter((_, i) => i !== index));
  };

  const sendMessage = async () => {
    if (!messageText.trim() && files.filter(f => f.uploaded).length === 0) {
      appendOutput('Please enter a message or attach files');
      return;
    }

    setLoading(true);
    try {
      const uploadedFiles = files.filter(f => f.uploaded && f.blob_id);
      
      appendOutput(`Sending message with ${uploadedFiles.length} attachments...`);
      
      const response = await chatApi.sendMessage({
        sender: senderName,
        text: messageText,
        attachment_blob_ids: uploadedFiles.map(f => f.blob_id!),
        attachment_names: uploadedFiles.map(f => f.file.name),
        attachment_sizes: uploadedFiles.map(f => f.file.size),
        attachment_content_types: uploadedFiles.map(f => f.file.type || null),
      });

      if (response.error) {
        appendOutput(`Error sending message: ${response.error.message}`);
      } else {
        appendOutput(`Message sent successfully! ID: ${response.data}`);
        setMessageText('');
        setFiles([]);
        await loadMessages();
        await loadStats();
      }
    } catch (error) {
      appendOutput(`Failed to send message: ${error}`);
    } finally {
      setLoading(false);
    }
  };

  const downloadAttachment = async (message: Message, attachmentIndex: number) => {
    try {
      const attachment = message.attachments[attachmentIndex];
      appendOutput(`Downloading attachment ${attachmentIndex} from message ${message.id}...`);
      
      // Step 1: Get decompressed blob ID from contract (lazy decompression)
      appendOutput(`Requesting decompressed blob ID for: ${attachment.compressed_blob_id}`);
      const decompressedResponse = await chatApi.getDecompressedBlobId(attachment.compressed_blob_id);

      if (decompressedResponse.error) {
        appendOutput(`Error getting decompressed blob ID: ${decompressedResponse.error.message}`);
        return;
      }

      const decompressedBlobId = decompressedResponse.data!;
      appendOutput(`Got decompressed blob ID: ${decompressedBlobId}`);
      
      // Step 2: Download the original file via HTTP
      appendOutput(`Downloading original file via HTTP...`);
      const blob = await blobClient.downloadBlob(decompressedBlobId);
      
      // Step 3: Create download link
      const url = URL.createObjectURL(blob);
      const link = document.createElement('a');
      link.href = url;
      link.download = attachment.original_name;
      document.body.appendChild(link);
      link.click();
      document.body.removeChild(link);
      URL.revokeObjectURL(url);
      
      appendOutput(`Successfully downloaded: ${attachment.original_name}`);
    } catch (error) {
      appendOutput(`Failed to download attachment: ${error}`);
    }
  };

  const toggleMessageExpansion = (messageId: number) => {
    setMessages(prev => prev.map(msg => 
      msg.id === messageId ? { ...msg, isExpanded: !msg.isExpanded } : msg
    ));
  };

  const clearMessages = async () => {
    try {
      const response = await chatApi.clearMessages();
      if (response.error) {
        appendOutput(`Error clearing messages: ${response.error.message}`);
      } else {
        appendOutput('All messages cleared');
        await loadMessages();
        await loadStats();
      }
    } catch (error) {
      appendOutput(`Failed to clear messages: ${error}`);
    }
  };

  const clearOutput = () => {
    setOutput('');
  };

  const formatFileSize = (bytes: number): string => {
    if (bytes === 0) return '0 Bytes';
    const k = 1024;
    const sizes = ['Bytes', 'KB', 'MB', 'GB'];
    const i = Math.floor(Math.log(bytes) / Math.log(k));
    return parseFloat((bytes / Math.pow(k, i)).toFixed(2)) + ' ' + sizes[i];
  };

  const formatTimestamp = (timestamp: number): string => {
    return new Date(timestamp / 1000000).toLocaleString();
  };

  const renderAttachment = (attachment: Attachment, messageId: number, index: number) => (
    <div key={index} className="attachment-item">
      <div className="attachment-info">
        <span className="attachment-name">{attachment.original_name}</span>
        <span className="attachment-size">
          {formatFileSize(attachment.original_size)} → {formatFileSize(attachment.compressed_size)}
          ({(attachment.compression_ratio * 100).toFixed(1)}% of original)
        </span>
      </div>
      <button
        onClick={() => downloadAttachment(messages.find(m => m.id === messageId)!, index)}
        className="download-btn"
      >
        Download
      </button>
    </div>
  );

  return (
    <div className="chat-container">
      <div className="header">
        <h1>Chat with Blob Attachments</h1>
        <div className="stats">
          <span>Messages: {stats.total_messages || 0}</span>
          <span>Attachments: {stats.total_attachments || 0}</span>
          <span>Compression Savings: {stats.compression_savings_percent || 0}%</span>
        </div>
      </div>

      <div className="main-content">
        <div className="chat-section">
          <div className="messages-container">
            {messages.map((message) => (
              <div key={message.id} className="message">
                <div className="message-header">
                  <span className="sender">{message.sender}</span>
                  <span className="timestamp">{formatTimestamp(message.timestamp)}</span>
                  <span className="message-id">#{message.id}</span>
                </div>
                
                <div className="message-text">{message.text}</div>
                
                {message.attachments.length > 0 && (
                  <div className="attachments">
                    <div 
                      className="attachments-header"
                      onClick={() => toggleMessageExpansion(message.id)}
                    >
                      📎 {message.attachments.length} attachment{message.attachments.length !== 1 ? 's' : ''}
                      <span className="expand-icon">{message.isExpanded ? '▼' : '▶'}</span>
                    </div>
                    
                    {message.isExpanded && (
                      <div className="attachments-list">
                        {message.attachments.map((attachment, index) => 
                          renderAttachment(attachment, message.id, index)
                        )}
                      </div>
                    )}
                  </div>
                )}
              </div>
            ))}
            <div ref={messagesEndRef} />
          </div>

          <div className="input-section">
            <div className="sender-input">
              <label>
                Sender:
                <input
                  type="text"
                  value={senderName}
                  onChange={(e) => setSenderName(e.target.value)}
                  placeholder="Your name"
                />
              </label>
            </div>

            <div 
              className={`file-drop-zone ${isDragOver ? 'drag-over' : ''}`}
              onDrop={handleDrop}
              onDragOver={handleDragOver}
              onDragLeave={handleDragLeave}
              onClick={() => fileInputRef.current?.click()}
            >
              <input
                ref={fileInputRef}
                type="file"
                multiple
                onChange={(e) => handleFileSelection(Array.from(e.target.files || []))}
                style={{ display: 'none' }}
              />
              <div className="drop-zone-content">
                📁 Drop files here or click to select
              </div>
            </div>

            {files.length > 0 && (
              <div className="selected-files">
                <h4>Selected Files:</h4>
                {files.map((fileUpload, index) => (
                  <div key={index} className="file-item">
                    <span className="file-name">{fileUpload.file.name}</span>
                    <span className="file-size">{formatFileSize(fileUpload.file.size)}</span>
                    
                    {fileUpload.uploading && (
                      <div className="upload-progress">
                        <div className="progress-bar">
                          <div 
                            className="progress-fill"
                            style={{ width: `${fileUpload.progress}%` }}
                          />
                        </div>
                        <span>{Math.round(fileUpload.progress)}%</span>
                      </div>
                    )}
                    
                    {fileUpload.uploaded && (
                      <span className="upload-status success">✅ Uploaded</span>
                    )}
                    
                    {fileUpload.error && (
                      <span className="upload-status error">❌ {fileUpload.error}</span>
                    )}
                    
                    <button 
                      onClick={() => removeFile(index)}
                      className="remove-file-btn"
                    >
                      ✕
                    </button>
                  </div>
                ))}
              </div>
            )}

            <div className="message-input">
              <textarea
                value={messageText}
                onChange={(e) => setMessageText(e.target.value)}
                placeholder="Type your message..."
                rows={3}
                onKeyDown={(e) => {
                  if (e.key === 'Enter' && !e.shiftKey) {
                    e.preventDefault();
                    sendMessage();
                  }
                }}
              />
              <div className="message-actions">
                <button 
                  onClick={sendMessage} 
                  disabled={loading || (!messageText.trim() && files.filter(f => f.uploaded).length === 0)}
                  className="send-btn"
                >
                  {loading ? 'Sending...' : 'Send Message'}
                </button>
              </div>
            </div>
          </div>
        </div>

        <div className="sidebar">
          <div className="controls">
            <button onClick={loadMessages} className="control-btn">
              Refresh Messages
            </button>
            <button onClick={loadStats} className="control-btn">
              Refresh Stats
            </button>
            <button onClick={clearMessages} className="control-btn danger">
              Clear All Messages
            </button>
            <button onClick={clearOutput} className="control-btn">
              Clear Output
            </button>
          </div>

          <div className="output-section">
            <h3>Output Log</h3>
            <textarea
              readOnly
              value={output}
              className="output-area"
            />
          </div>
        </div>
      </div>

      <style>{`
        .chat-container {
          max-width: 1400px;
          margin: 0 auto;
          padding: 20px;
          font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', 'Roboto', sans-serif;
        }

        .header {
          display: flex;
          justify-content: space-between;
          align-items: center;
          margin-bottom: 20px;
          padding-bottom: 10px;
          border-bottom: 2px solid #e0e0e0;
        }

        .header h1 {
          margin: 0;
          color: #333;
        }

        .stats {
          display: flex;
          gap: 20px;
          font-weight: 500;
          color: #666;
        }

        .main-content {
          display: grid;
          grid-template-columns: 2fr 1fr;
          gap: 20px;
          height: calc(100vh - 120px);
        }

        .chat-section {
          display: flex;
          flex-direction: column;
          background: white;
          border: 1px solid #ddd;
          border-radius: 8px;
          overflow: hidden;
        }

        .messages-container {
          flex: 1;
          overflow-y: auto;
          padding: 20px;
          background: #f8f9fa;
        }

        .message {
          background: white;
          border: 1px solid #e0e0e0;
          border-radius: 8px;
          margin-bottom: 16px;
          padding: 16px;
          box-shadow: 0 1px 3px rgba(0,0,0,0.1);
        }

        .message-header {
          display: flex;
          align-items: center;
          gap: 12px;
          margin-bottom: 8px;
          font-size: 14px;
        }

        .sender {
          font-weight: 600;
          color: #2563eb;
        }

        .timestamp {
          color: #6b7280;
        }

        .message-id {
          color: #9ca3af;
          font-family: monospace;
        }

        .message-text {
          margin-bottom: 12px;
          line-height: 1.5;
          white-space: pre-wrap;
        }

        .attachments {
          border-top: 1px solid #e5e7eb;
          padding-top: 12px;
        }

        .attachments-header {
          cursor: pointer;
          display: flex;
          align-items: center;
          gap: 8px;
          font-weight: 500;
          color: #374151;
          user-select: none;
        }

        .expand-icon {
          margin-left: auto;
          color: #9ca3af;
        }

        .attachments-list {
          margin-top: 8px;
          padding-left: 20px;
        }

        .attachment-item {
          display: flex;
          align-items: center;
          justify-content: space-between;
          padding: 8px 0;
          border-bottom: 1px solid #f3f4f6;
        }

        .attachment-info {
          display: flex;
          flex-direction: column;
          gap: 4px;
        }

        .attachment-name {
          font-weight: 500;
        }

        .attachment-size {
          font-size: 12px;
          color: #6b7280;
        }

        .download-btn {
          background: #3b82f6;
          color: white;
          border: none;
          padding: 4px 12px;
          border-radius: 4px;
          cursor: pointer;
          font-size: 12px;
        }

        .download-btn:hover {
          background: #2563eb;
        }

        .input-section {
          border-top: 1px solid #e0e0e0;
          padding: 20px;
          background: white;
        }

        .sender-input {
          margin-bottom: 16px;
        }

        .sender-input label {
          display: flex;
          align-items: center;
          gap: 8px;
          font-weight: 500;
        }

        .sender-input input {
          padding: 8px 12px;
          border: 1px solid #d1d5db;
          border-radius: 4px;
          font-size: 14px;
        }

        .file-drop-zone {
          border: 2px dashed #d1d5db;
          border-radius: 8px;
          padding: 24px;
          text-align: center;
          cursor: pointer;
          margin-bottom: 16px;
          transition: all 0.2s;
        }

        .file-drop-zone:hover,
        .file-drop-zone.drag-over {
          border-color: #3b82f6;
          background: #eff6ff;
        }

        .drop-zone-content {
          color: #6b7280;
          font-size: 16px;
        }

        .selected-files {
          margin-bottom: 16px;
        }

        .selected-files h4 {
          margin: 0 0 12px 0;
          color: #374151;
        }

        .file-item {
          display: flex;
          align-items: center;
          gap: 12px;
          padding: 8px 0;
          border-bottom: 1px solid #f3f4f6;
        }

        .file-name {
          font-weight: 500;
          flex: 1;
        }

        .file-size {
          color: #6b7280;
          font-size: 14px;
        }

        .upload-progress {
          display: flex;
          align-items: center;
          gap: 8px;
        }

        .progress-bar {
          width: 80px;
          height: 6px;
          background: #f3f4f6;
          border-radius: 3px;
          overflow: hidden;
        }

        .progress-fill {
          height: 100%;
          background: #3b82f6;
          transition: width 0.3s;
        }

        .upload-status.success {
          color: #059669;
          font-size: 14px;
        }

        .upload-status.error {
          color: #dc2626;
          font-size: 14px;
        }

        .remove-file-btn {
          background: #ef4444;
          color: white;
          border: none;
          width: 24px;
          height: 24px;
          border-radius: 50%;
          cursor: pointer;
          font-size: 12px;
          display: flex;
          align-items: center;
          justify-content: center;
        }

        .message-input {
          position: relative;
        }

        .message-input textarea {
          width: 100%;
          padding: 12px;
          border: 1px solid #d1d5db;
          border-radius: 8px;
          resize: vertical;
          font-family: inherit;
          font-size: 14px;
        }

        .message-actions {
          display: flex;
          justify-content: flex-end;
          margin-top: 8px;
        }

        .send-btn {
          background: #3b82f6;
          color: white;
          border: none;
          padding: 8px 16px;
          border-radius: 6px;
          cursor: pointer;
          font-weight: 500;
        }

        .send-btn:disabled {
          background: #9ca3af;
          cursor: not-allowed;
        }

        .send-btn:not(:disabled):hover {
          background: #2563eb;
        }

        .sidebar {
          display: flex;
          flex-direction: column;
          gap: 20px;
        }

        .controls {
          display: flex;
          flex-direction: column;
          gap: 8px;
        }

        .control-btn {
          padding: 8px 16px;
          border: 1px solid #d1d5db;
          border-radius: 6px;
          background: white;
          cursor: pointer;
          font-size: 14px;
        }

        .control-btn:hover {
          background: #f9fafb;
        }

        .control-btn.danger {
          background: #fef2f2;
          border-color: #fecaca;
          color: #dc2626;
        }

        .control-btn.danger:hover {
          background: #fee2e2;
        }

        .output-section {
          flex: 1;
        }

        .output-section h3 {
          margin: 0 0 12px 0;
          color: #374151;
        }

        .output-area {
          width: 100%;
          height: 300px;
          padding: 12px;
          border: 1px solid #d1d5db;
          border-radius: 6px;
          font-family: monospace;
          font-size: 12px;
          background: #f9fafb;
          resize: vertical;
        }
      `}</style>
    </div>
  );
}
