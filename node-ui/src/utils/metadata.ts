export interface AppMetadata {
  contractAppId: string;
}

export function createAppMetadata(contractAppId: string): number[] {
  var appMetadata: AppMetadata = {
    contractAppId,
  };

  return Array.from(new TextEncoder().encode(JSON.stringify(appMetadata)));
}

export function parseAppMetadata(metadata: number[]): AppMetadata | null {
  try {
    if (metadata.length === 0) {
      return null;
    }

    var appMetadata: AppMetadata = JSON.parse(
      new TextDecoder().decode(new Uint8Array(metadata)),
    );
    return appMetadata;
  } catch (e) {
    return null;
  }
}
