interface WalletRedirectParams {
  accountId?: string | undefined;
  publicKey?: string | undefined;
  address?: string | undefined;
  chainId?: string | undefined;
  error?: string | undefined;
}

export const handleWalletRedirect = (): WalletRedirectParams => {
  const searchParams = new URLSearchParams(window.location.search);
  const params: WalletRedirectParams = {};

  // NEAR wallet parameters
  if (searchParams.has('account_id')) {
    params.accountId = searchParams.get('account_id') || undefined;
    
    // Handle all_keys parameter which contains the public key
    const allKeys = searchParams.get('all_keys');
    if (allKeys) {
      // Extract the public key from the all_keys string (format: "ed25519:public_key")
      const publicKeyMatch = allKeys.match(/ed25519:([^,]+)/);
      if (publicKeyMatch) {
        params.publicKey = publicKeyMatch[1];
      }
    }
  }

  // MetaMask parameters
  if (searchParams.has('address')) {
    params.address = searchParams.get('address') || undefined;
    params.chainId = searchParams.get('chainId') || undefined;
  }

  // Error parameter (common for both)
  if (searchParams.has('error')) {
    params.error = searchParams.get('error') || undefined;
  }

  // // Clear URL parameters without reloading the page
  // if (searchParams.toString()) {
  //   const newUrl = window.location.pathname + window.location.hash;
  //   window.history.replaceState({}, '', newUrl);
  // }

  return params;
}; 