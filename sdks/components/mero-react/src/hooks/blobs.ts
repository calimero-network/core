/**
 * An app's blobs (`/apps/{app}/blobs`). Blobs belong to an app instance now,
 * so every hook takes the app (default: the provider's). `blobId` is base58,
 * as apps print it; the node takes either form.
 */

import { useCallback, useEffect, useState } from 'react';
import type { BlobInfo } from '@calimero-network/mero-js';
import { useAppId, useClientAction, useResource, useSignedInClient, type MutationOptions } from './internal';

export interface BlobHookOptions {
  appId?: string | null;
}

export interface UseBlobUrlOptions extends BlobHookOptions {
  /** The object URL's type (default: none). */
  mimeType?: string;
}

async function bytesOf(data: Uint8Array | Blob | ArrayBuffer): Promise<Uint8Array> {
  if (data instanceof Uint8Array) return data;
  if (data instanceof ArrayBuffer) return new Uint8Array(data);
  return new Uint8Array(await data.arrayBuffer());
}

/** Upload bytes, a `Blob` or a `File`; resolves `{ blobId, id, size }`. */
export function useUploadBlob(options: BlobHookOptions & MutationOptions = {}) {
  const id = useAppId(options.appId);
  const { withClient, ...state } = useClientAction(options);
  const upload = useCallback(
    (data: Uint8Array | Blob | ArrayBuffer) =>
      withClient(async (c): Promise<{ blobId: string; id: string; size: number }> => {
        if (!id) throw new Error('No app to store the blob in.');
        return c.apps.blobUpload({ app: id, body: await bytesOf(data), contentType: 'application/octet-stream' });
      }),
    [withClient, id],
  );
  return { upload, ...state };
}

/** A blob's size and ids. */
export function useBlobInfo(blobId?: string | null, options: BlobHookOptions = {}) {
  const client = useSignedInClient();
  const id = useAppId(options.appId);
  return useResource<BlobInfo | null>(
    client && id && blobId ? () => client.apps.blobInfo({ app: id, id: blobId }) : null,
    null,
    [client, id, blobId],
  );
}

/** The app's blobs. */
export function useBlobs(appId?: string | null) {
  const client = useSignedInClient();
  const id = useAppId(appId);
  return useResource<BlobInfo[]>(
    client && id ? async () => (await client.apps.blobList({ app: id })).blobs : null,
    [],
    [client, id],
  );
}

/**
 * An object URL for a blob, for an `<img>` or a download link. The bytes come
 * over the sealed channel, so a plain URL to the node would not work. Revoked
 * when the blob changes or the component unmounts.
 */
export function useBlobUrl(blobId?: string | null, options: UseBlobUrlOptions = {}) {
  const client = useSignedInClient();
  const id = useAppId(options.appId);
  const { mimeType } = options;
  const bytes = useResource<Uint8Array | null>(
    client && id && blobId ? () => client.downloadBlob(id, blobId) : null,
    null,
    [client, id, blobId],
  );
  const [url, setUrl] = useState<string | null>(null);
  useEffect(() => {
    if (!bytes.data || typeof URL.createObjectURL !== 'function') {
      setUrl(null);
      return;
    }
    const made = URL.createObjectURL(new Blob([bytes.data as BlobPart], mimeType ? { type: mimeType } : undefined));
    setUrl(made);
    return () => URL.revokeObjectURL(made);
  }, [bytes.data, mimeType]);
  return { url, loading: bytes.loading, error: bytes.error, failure: bytes.failure, refetch: bytes.refetch };
}

/** Delete a blob. */
export function useDeleteBlob(options: BlobHookOptions & MutationOptions = {}) {
  const id = useAppId(options.appId);
  const { withClient, ...state } = useClientAction(options);
  const deleteBlob = useCallback(
    (blobId: string) =>
      withClient(async (c) => {
        if (!id) throw new Error('No app.');
        await c.apps.blobDelete({ app: id, id: blobId });
        return true;
      }),
    [withClient, id],
  );
  return { deleteBlob, ...state };
}
