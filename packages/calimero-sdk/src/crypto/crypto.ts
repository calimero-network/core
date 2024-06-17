import { unmarshalPrivateKey } from '@libp2p/crypto/keys';
import { PrivateKey } from '@libp2p/interface';
import bs58 from 'bs58';
import { WalletType } from '../api/nodeApi';
import { ClientKey } from '../types/storage';
import { getStorageClientKey } from '../storage/storage';
import { Header } from '../api/httpClient';

export async function createAuthHeader(
  payload: string,
): Promise<Header[] | null> {
  const privateKey: PrivateKey = await getPrivateKey();

  if (!privateKey) {
    return null;
  }

  const encoder = new TextEncoder();
  const contentBuff = encoder.encode(payload);

  const signing_key = bs58.encode(privateKey.public.bytes);

  const hashBuffer = await crypto.subtle.digest('SHA-256', contentBuff);
  const hashArray = new Uint8Array(hashBuffer);

  const signature = await privateKey.sign(hashArray);
  const signatureBase58 = bs58.encode(signature);
  const contentBase58 = bs58.encode(hashArray);

  const headers: Header[] = [
    { key: 'wallet_type', value: JSON.stringify(WalletType.NEAR) },
    { key: 'signing_key', value: signing_key },
    { key: 'signature', value: signatureBase58 },
    { key: 'challenge', value: contentBase58 },
  ];

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
