import { unmarshalPrivateKey } from '@libp2p/crypto/keys';
import { PrivateKey } from '@libp2p/interface';
import bs58 from 'bs58';
import { WalletType } from '../api/nodeApi';
import { ClientKey } from '../types/storage';
import { getStorageClientKey } from '../storage/storage';

export interface Header {
  [key: string]: string;
}

export async function createAuthHeader(
  payload: string,
  contextId: string | null = null,
): Promise<Header | null> {
  const privateKey: PrivateKey = await getPrivateKey();

  if (!privateKey) {
    return null;
  }

  const encoder = new TextEncoder();
  const contentBuff = encoder.encode(payload);

  const signingKey = bs58.encode(privateKey.public.bytes);

  const hashBuffer = await crypto.subtle.digest('SHA-256', contentBuff);
  const hashArray = new Uint8Array(hashBuffer);

  const signature = await privateKey.sign(hashArray);
  const signatureBase58 = bs58.encode(signature);
  const contentBase58 = bs58.encode(hashArray);

  const headers: Header = {
    wallet_type: JSON.stringify(WalletType.NEAR),
    signing_key: signingKey,
    signature: signatureBase58,
    challenge: contentBase58,
    context_id: contextId,
  };

  return headers;
}

export async function getPrivateKey(): Promise<PrivateKey | null> {
  try {
    const clientKey: ClientKey | null = getStorageClientKey();
    if (!clientKey) {
      return null;
    }
    return await unmarshalPrivateKey(bs58.decode(clientKey.privateKey));
  } catch (error) {
    console.error('Error extracting private key:', error);
    return null;
  }
}
