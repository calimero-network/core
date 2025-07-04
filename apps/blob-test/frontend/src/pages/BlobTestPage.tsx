import React, { useState, useEffect } from 'react';
import styled from 'styled-components';
import { BlobApiDataSource } from '../api/dataSource/BlobApiDataSource';
import { BlobMetadata } from '../api/blobApi';

const PageContainer = styled.div`
  display: flex;
  flex-direction: column;
  height: 100vh;
  width: 100vw;
  background-color: #111111;
  padding: 2rem;
  overflow-y: auto;
`;

const Header = styled.div`
  text-align: center;
  margin-bottom: 2rem;
  
  h1 {
    color: white;
    font-size: 2.5rem;
    margin-bottom: 0.5rem;
  }
  
  p {
    color: #aaa;
    font-size: 1.1rem;
  }
`;

const Grid = styled.div`
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(400px, 1fr));
  gap: 2rem;
  max-width: 1200px;
  margin: 0 auto;
  width: 100%;
`;

const Card = styled.div`
  background: #222;
  border-radius: 8px;
  padding: 1.5rem;
  border: 1px solid #333;
  
  h3 {
    color: white;
    margin-top: 0;
    margin-bottom: 1rem;
    font-size: 1.3rem;
  }
  
  .description {
    color: #aaa;
    margin-bottom: 1rem;
    font-size: 0.9rem;
  }
`;

const Button = styled.button`
  background: #5dbb63;
  color: white;
  border: none;
  border-radius: 6px;
  padding: 0.75rem 1.5rem;
  font-size: 1rem;
  cursor: pointer;
  margin: 0.25rem;
  transition: background-color 0.2s;
  
  &:hover {
    background: #4a9950;
  }
  
  &:disabled {
    background: #555;
    cursor: not-allowed;
  }
`;

const Input = styled.input`
  background: #333;
  color: white;
  border: 1px solid #555;
  border-radius: 4px;
  padding: 0.5rem;
  margin: 0.25rem;
  width: 100%;
  
  &:focus {
    outline: none;
    border-color: #5dbb63;
  }
`;

const OutputBox = styled.div`
  background: #1a1a1a;
  border: 1px solid #444;
  border-radius: 4px;
  padding: 1rem;
  margin-top: 1rem;
  max-height: 200px;
  overflow-y: auto;
  white-space: pre-wrap;
  font-family: monospace;
  font-size: 0.9rem;
  color: #ccc;
`;

const StatusIndicator = styled.div<{ $success?: boolean }>`
  padding: 0.5rem;
  border-radius: 4px;
  margin: 0.5rem 0;
  background: ${props => props.$success ? '#155724' : '#721c24'};
  color: ${props => props.$success ? '#d4edda' : '#f8d7da'};
  border: 1px solid ${props => props.$success ? '#c3e6cb' : '#f5c6cb'};
`;

const FileInputWrapper = styled.div`
  margin: 0.5rem 0;
  
  input[type="file"] {
    background: #333;
    color: white;
    border: 1px solid #555;
    border-radius: 4px;
    padding: 0.5rem;
    width: 100%;
  }
`;

const BlobListItem = styled.div`
  background: #2a2a2a;
  border: 1px solid #444;
  border-radius: 4px;
  padding: 0.75rem;
  margin: 0.5rem 0;
  
  .blob-name {
    font-weight: bold;
    color: #5dbb63;
    margin-bottom: 0.25rem;
  }
  
  .blob-id {
    color: #aaa;
    font-size: 0.8rem;
    font-family: monospace;
    margin-bottom: 0.25rem;
  }
  
  .blob-meta {
    color: #ccc;
    font-size: 0.9rem;
    margin-bottom: 0.5rem;
  }
  
  .blob-actions {
    display: flex;
    gap: 0.5rem;
  }
`;

export default function BlobTestPage() {
  const [blobApi] = useState(new BlobApiDataSource());
  const [loading, setLoading] = useState(false);
  const [output, setOutput] = useState<string>('');
  const [blobs, setBlobs] = useState<Record<string, BlobMetadata>>({});
  
  // REST API Upload
  const [uploadFile, setUploadFile] = useState<File | null>(null);
  const [uploadStatus, setUploadStatus] = useState<string>('');
  const [lastUploadedBlobId, setLastUploadedBlobId] = useState<string>('');
  
  // JSON RPC Register  
  const [registerName, setRegisterName] = useState('');
  const [registerBlobId, setRegisterBlobId] = useState('');
  const [registerSize, setRegisterSize] = useState<string>('');
  
  // JSON RPC Read
  const [readName, setReadName] = useState('');
  
  // REST API Download
  const [downloadBlobId, setDownloadBlobId] = useState('');

  useEffect(() => {
    loadBlobList();
  }, []);

  const appendOutput = (message: string) => {
    setOutput(prev => `${prev}${new Date().toLocaleTimeString()}: ${message}\n`);
  };

  const loadBlobList = async () => {
    try {
      const response = await blobApi.listBlobs();
      if (response.data) {
        setBlobs(response.data);
        appendOutput(`Loaded ${Object.keys(response.data).length} blobs`);
      } else if (response.error) {
        appendOutput(`Error loading blobs: ${response.error.message}`);
      }
    } catch (error) {
      appendOutput(`Error loading blobs: ${error}`);
    }
  };

  // REST API Upload
  const uploadViaRestApi = async () => {
    if (!uploadFile) {
      appendOutput('Please select a file to upload');
      return;
    }

    setLoading(true);
    setUploadStatus('Uploading...');
    
    try {
      appendOutput(`Uploading "${uploadFile.name}" (${uploadFile.size} bytes) via REST API...`);
      
      const response = await blobApi.uploadBlob(uploadFile);
      
      if (response.data) {
        const blobId = response.data.blob_id;
        setLastUploadedBlobId(blobId);
        setRegisterBlobId(blobId); // Auto-populate for easy registration
        setRegisterSize(uploadFile.size.toString());
        setDownloadBlobId(blobId); // Auto-populate for easy download
        
        setUploadStatus(`Success! Blob ID: ${blobId}`);
        appendOutput(`Uploaded successfully! Blob ID: ${blobId}`);
      } else if (response.error) {
        setUploadStatus(`Error: ${response.error.message}`);
        appendOutput(`Upload failed: ${response.error.message}`);
      }
      
    } catch (error) {
      setUploadStatus(`Error: ${error}`);
      appendOutput(`Upload failed: ${error}`);
    } finally {
      setLoading(false);
    }
  };

  // JSON RPC Register
  const registerBlob = async () => {
    if (!registerName || !registerBlobId || !registerSize) {
      appendOutput('Please fill in name, blob ID, and size');
      return;
    }

    setLoading(true);
    
    try {
      appendOutput(`Registering blob with name="${registerName}", blob_id="${registerBlobId}", size=${registerSize}...`);
      
      await blobApi.registerBlob({
        name: registerName,
        blob_id: registerBlobId,
        size: parseInt(registerSize),
        content_type: uploadFile?.type || undefined
      });
      
      appendOutput(`Registered blob "${registerName}" successfully!`);
      setReadName(registerName); // Auto-populate for easy reading
      
      // Reload blob list
      await loadBlobList();
      
    } catch (error) {
      appendOutput(`Registration failed: ${error}`);
    } finally {
      setLoading(false);
    }
  };

  // JSON RPC Read
  const readBlob = async () => {
    if (!readName) {
      appendOutput('Please enter a blob name to read');
      return;
    }

    setLoading(true);
    
    try {
      appendOutput(`Reading blob "${readName}" via JSON RPC...`);
      
      const response = await blobApi.readBlob({ name: readName });
      
      if (response.data) {
        appendOutput(`Successfully read "${readName}" (${response.data.length} bytes)`);
      } else if (response.error) {
        appendOutput(`Read failed: ${response.error.message}`);
      } else {
        appendOutput(`Failed to read blob "${readName}"`);
      }
      
    } catch (error) {
      appendOutput(`Read failed: ${error}`);
    } finally {
      setLoading(false);
    }
  };

  // REST API Download
  const downloadViaRestApi = async () => {
    if (!downloadBlobId) {
      appendOutput('Please enter a blob ID to download');
      return;
    }

    setLoading(true);
    
    try {
      appendOutput(`Downloading blob "${downloadBlobId}" via REST API...`);
      
      const blob = await blobApi.downloadBlob(downloadBlobId);
      
      // Determine proper filename and extension
      let filename = `blob-${downloadBlobId.slice(0, 8)}`;
      
      // Try to get file extension from blob type
      if (blob.type) {
        const extension = blob.type.split('/')[1];
        if (extension && extension !== 'octet-stream') {
          filename += `.${extension}`;
        }
      }
      
      // Create download link
      const url = URL.createObjectURL(blob);
      const a = document.createElement('a');
      a.href = url;
      a.download = filename;
      document.body.appendChild(a);
      a.click();
      document.body.removeChild(a);
      URL.revokeObjectURL(url);
      
      appendOutput(`Successfully downloaded "${downloadBlobId}" (${blob.size} bytes)`);
      
    } catch (error) {
      appendOutput(`Download failed: ${error}`);
    } finally {
      setLoading(false);
    }
  };

  // Unregister Blob
  const unregisterBlob = async (name: string) => {
    if (!confirm(`Are you sure you want to unregister blob "${name}"?`)) {
      return;
    }
    
    setLoading(true);
    
    try {
      appendOutput(`Unregistering blob "${name}"...`);
      
      const response = await blobApi.unregisterBlob(name);
      
      if (response.error) {
        appendOutput(`Unregister failed: ${response.error.message}`);
      } else {
        appendOutput(`Successfully unregistered blob "${name}"`);
        // Reload blob list
        await loadBlobList();
      }
      
    } catch (error) {
      appendOutput(`Unregister failed: ${error}`);
    } finally {
      setLoading(false);
    }
  };

  const clearOutput = () => {
    setOutput('');
  };

  return (
    <PageContainer>
      <Header>
        <h1>Calimero Blob API Demo</h1>
        <p>REST API Upload/Download + JSON RPC Register/Read</p>
      </Header>

      <Grid>
        {/* REST API Upload */}
        <Card>
          <h3>1. REST API Upload</h3>
          <div className="description">
            Upload files directly to blob storage via REST API
          </div>
          
          <FileInputWrapper>
            <input
              type="file"
              onChange={(e) => setUploadFile(e.target.files?.[0] || null)}
              disabled={loading}
            />
          </FileInputWrapper>
          
          <Button onClick={uploadViaRestApi} disabled={!uploadFile || loading}>
            Upload via REST API
          </Button>
          
          {uploadStatus && (
            <StatusIndicator $success={uploadStatus.includes('Success')}>
              {uploadStatus}
            </StatusIndicator>
          )}
        </Card>

        {/* JSON RPC Register */}
        <Card>
          <h3>2. JSON RPC Register</h3>
          <div className="description">
            Register uploaded blobs with names and metadata via JSON RPC
          </div>
          
          <Input
            type="text"
            placeholder="Blob name"
            value={registerName}
            onChange={(e) => setRegisterName(e.target.value)}
            disabled={loading}
          />
          
          <Input
            type="text"
            placeholder="Blob ID (from upload)"
            value={registerBlobId}
            onChange={(e) => setRegisterBlobId(e.target.value)}
            disabled={loading}
          />
          
          <Input
            type="number"
            placeholder="Size in bytes"
            value={registerSize}
            onChange={(e) => setRegisterSize(e.target.value)}
            disabled={loading}
          />
          
          <Button onClick={registerBlob} disabled={!registerName || !registerBlobId || !registerSize || loading}>
            Register via JSON RPC
          </Button>
        </Card>

        {/* JSON RPC Read */}
        <Card>
          <h3>3. JSON RPC Read</h3>
          <div className="description">
            Read blob data by name via JSON RPC
          </div>
          
          <Input
            type="text"
            placeholder="Blob name"
            value={readName}
            onChange={(e) => setReadName(e.target.value)}
            disabled={loading}
          />
          
          <Button onClick={readBlob} disabled={!readName || loading}>
            Read via JSON RPC
          </Button>
        </Card>

        {/* REST API Download */}
        <Card>
          <h3>4. REST API Download</h3>
          <div className="description">
            Download blobs directly by ID via REST API
          </div>
          
          <Input
            type="text"
            placeholder="Blob ID"
            value={downloadBlobId}
            onChange={(e) => setDownloadBlobId(e.target.value)}
            disabled={loading}
          />
          
          <Button onClick={downloadViaRestApi} disabled={!downloadBlobId || loading}>
            Download via REST API
          </Button>
        </Card>

        {/* Registered Blobs */}
        <Card>
          <h3>Registered Blobs</h3>
          <div className="description">
            Blobs registered via JSON RPC with names and metadata
          </div>
          
          <Button onClick={loadBlobList} disabled={loading}>
            Refresh List
          </Button>
          
          {Object.entries(blobs).map(([name, metadata]) => (
            <BlobListItem key={name}>
              <div className="blob-name">{name}</div>
              <div className="blob-id">ID: {metadata.blob_id}</div>
              <div className="blob-meta">
                Size: {metadata.size} bytes | Type: {metadata.content_type || 'unknown'}
              </div>
              <div className="blob-actions">
                <Button onClick={() => setReadName(name)}>
                  Select for Read
                </Button>
                <Button onClick={() => setDownloadBlobId(metadata.blob_id)}>
                  Select for Download
                </Button>
                <Button 
                  onClick={() => unregisterBlob(name)}
                  disabled={loading}
                  style={{ background: '#dc3545' }}
                >
                  Unregister
                </Button>
              </div>
            </BlobListItem>
          ))}
          
          {Object.keys(blobs).length === 0 && (
            <div style={{ color: '#aaa', fontStyle: 'italic', padding: '1rem' }}>
              No registered blobs yet
            </div>
          )}
        </Card>

        {/* Output Log */}
        <Card>
          <h3>Activity Log</h3>
          <div className="description">
            Real-time log of all operations
          </div>
          
          <Button onClick={clearOutput}>Clear Log</Button>
          
          <OutputBox>
            {output || 'No activity yet...'}
          </OutputBox>
        </Card>
      </Grid>
    </PageContainer>
  );
} 