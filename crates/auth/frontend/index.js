// Import the NEAR API JS and wallet selector packages
import * as nearAPI from 'near-api-js';
import { setupWalletSelector } from '@near-wallet-selector/core';
import { setupMyNearWallet } from '@near-wallet-selector/my-near-wallet';

// Initialize the NEAR connection
async function initNear() {
  // Configure connection to the NEAR testnet
  const config = {
    networkId: 'testnet',
    nodeUrl: 'https://rpc.testnet.near.org',
    walletUrl: 'https://wallet.testnet.near.org',
    helperUrl: 'https://helper.testnet.near.org',
    explorerUrl: 'https://explorer.testnet.near.org',
  };

  // Initialize wallet selector with modules
  const selector = await setupWalletSelector({
    network: config.networkId,
    modules: [setupMyNearWallet()],
  });

  // Initialize connection to the NEAR blockchain
  const near = await nearAPI.connect(config);
  
  return { near, selector };
}

// Function to generate a random string nonce
function generateRandomString(length = 32) {
  const characters = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789';
  let result = '';
  const charactersLength = characters.length;
  for (let i = 0; i < length; i++) {
    result += characters.charAt(Math.floor(Math.random() * charactersLength));
  }
  return result;
}

// Function to fetch an authentication challenge from the server
async function fetchChallenge() {
  try {
    const response = await fetch('/auth/challenge?provider=near_wallet', {
      method: 'GET',
      headers: {
        'Content-Type': 'application/json'
      }
    });
    
    if (!response.ok) {
      throw new Error(`Failed to fetch challenge: ${response.statusText}`);
    }
    
    return await response.json();
  } catch (error) {
    console.error('Error fetching challenge:', error);
    throw error;
  }
}

// Function to submit the signed message to get a token
async function submitSignedChallenge(accountId, publicKey, signature, message, clientName) {
  try {
    // Validate inputs
    if (!publicKey) throw new Error('Public key is required');
    if (!signature) throw new Error('Signature is required');
    if (!message) throw new Error('Message is required');
    if (!clientName) throw new Error('Client name is required');
    
    // Ensure signature is properly base64 encoded
    // NEAR wallet signature might already be base64 encoded
    // but we ensure it's in the correct format
    let processedSignature = signature;
    try {
      // Check if it's already base64 by trying to decode
      atob(signature); // Just check if it decodes, no need to store the result
      // If it decoded successfully, it's already base64
    } catch (e) {
      // If it fails, we need to encode it
      console.log('Signature needs encoding, encoding to base64');
      // If the signature is a Uint8Array or similar, convert it to base64
      if (signature instanceof Uint8Array) {
        processedSignature = btoa(String.fromCharCode.apply(null, signature));
      } else {
        processedSignature = btoa(signature);
      }
    }
    
    console.log('Submitting with publicKey:', publicKey);
    console.log('Submitting with signature (first 20 chars):', processedSignature.substring(0, 20) + '...');
    
    // Prepare request payload - make account ID optional
    const payload = {
      auth_method: 'near_wallet',
      public_key: publicKey,
      client_name: clientName,
      permissions: ['read', 'write'],
      timestamp: Date.now(),
      signature: processedSignature,
      message: message
    };
    
    // Only include account ID if it's provided
    if (accountId) {
      payload.wallet_address = accountId;
      console.log('Including accountId:', accountId);
    }
    
    const response = await fetch('/auth/token', {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json'
      },
      body: JSON.stringify(payload)
    });
    
    if (!response.ok) {
      const errorData = await response.json();
      throw new Error(`Token request failed: ${errorData.error || response.statusText}`);
    }
    
    return await response.json();
  } catch (error) {
    console.error('Error submitting signed challenge:', error);
    throw error;
  }
}

// Function to sign a message with NEAR wallet
async function signWithNear() {
  try {
    // Show loading indicator
    const loadingElement = document.getElementById('loading');
    if (loadingElement) {
      loadingElement.className = 'loading active';
    }
    
    // Get challenge from server
    const challenge = await fetchChallenge();
    console.log('Received challenge:', challenge);
    
    // Initialize NEAR connection
    console.log('Initializing NEAR connection...');
    const { selector } = await initNear();
    console.log('NEAR connection initialized successfully');
    
    // Get wallet and account info
    console.log('Connecting to wallet...');
    const wallet = await selector.wallet('my-near-wallet');
    console.log('Wallet connected');
    
    // We'll make account ID optional, only used for display
    let accountId = null;
    try {
      console.log('Getting accounts...');
      const accounts = await wallet.getAccounts();
      console.log('Accounts received:', accounts);
      
      if (accounts && accounts.length > 0) {
        accountId = accounts[0].accountId;
        console.log('Found account ID:', accountId);
      }
    } catch (accountError) {
      // No account found, but we can still proceed with just the public key
      console.warn('Could not retrieve account ID:', accountError);
      console.log('Proceeding with public key authentication only');
    }
    
    // Prepare parameters for signing
    const message = challenge.message; // Keep as string, don't encode to bytes
    const nonce = Buffer.from(generateRandomString(32));
    const recipient = 'calimero-auth-frontend';
    const callbackUrl = window.location.href;
    
    console.log('Preparing to sign message with NEAR wallet');
    console.log('Message:', message);
    console.log('Recipient:', recipient);
    console.log('Nonce (first 10 chars):', nonce.toString('hex').substring(0, 10) + '...');
    console.log('Callback URL:', callbackUrl);
    
    // Sign the message
    console.log('Calling wallet.signMessage...');
    const result = await wallet.signMessage({
      message, // Pass as string, not as Uint8Array
      nonce,
      recipient,
      callbackUrl
    });
    
    console.log('Signature result received:', result);
    
    // Verify we have the necessary data
    if (!result || !result.signature) {
      throw new Error('The wallet did not return a valid signature');
    }
    
    if (!result.publicKey) {
      throw new Error('The wallet did not return a public key');
    }
    
    // Extract the public key and signature from the result
    // Handle different response formats from different wallet versions
    let publicKey = result.publicKey;
    let signature = result.signature;
    
    // Some wallet versions return data differently
    if (typeof result.data === 'object' && result.data) {
      if (result.data.publicKey) publicKey = result.data.publicKey;
      if (result.data.signature) signature = result.data.signature;
    }
    
    // After successful signing, submit to the token endpoint
    console.log('Submitting signed challenge to server...');
    const clientName = `calimero-client-${Date.now().toString(36)}`;
    const tokenResponse = await submitSignedChallenge(
      accountId, // This can now be null
      publicKey,
      signature,
      message,
      clientName
    );
    
    console.log('Authentication successful!', tokenResponse);
    
    // Store tokens in localStorage for future use
    if (tokenResponse.access_token && typeof window !== 'undefined' && window.localStorage) {
      window.localStorage.setItem('accessToken', tokenResponse.access_token);
      window.localStorage.setItem('refreshToken', tokenResponse.refresh_token);
      window.localStorage.setItem('tokenExpiry', Date.now() + (tokenResponse.expires_in * 1000));
      window.localStorage.setItem('clientId', tokenResponse.client_id);
      
      // Update UI to show successful authentication
      const statusElement = document.getElementById('auth-status');
      if (statusElement) {
        // Use account ID if available, or public key if not
        const userIdentifier = accountId || publicKey.substring(0, 10) + '...';
        statusElement.textContent = `Authenticated as: ${userIdentifier}`;
        statusElement.className = 'success';
      }
      
      // Update account info display
      const accountInfoElement = document.getElementById('account-info');
      const accountIdElement = document.getElementById('account-id');
      if (accountInfoElement && accountIdElement) {
        accountInfoElement.style.display = 'block';
        // Use account ID if available, or public key if not
        accountIdElement.textContent = accountId || `Public Key: ${publicKey.substring(0, 15)}...`;
      }
      
      // Show sign out button
      const signOutButton = document.getElementById('sign-out-button');
      const signInButton = document.getElementById('sign-in-button');
      if (signOutButton && signInButton) {
        signOutButton.style.display = 'inline-block';
        signInButton.style.display = 'none';
      }
    }
  } catch (error) {
    console.error('Error during authentication process:', error);
    console.error('Error details:', error.message);
    console.error('Error stack:', error.stack);
    
    // Let the user know something went wrong
    const statusElement = document.getElementById('auth-status');
    if (statusElement) {
      let errorMessage = error.message;
      
      // Provide more helpful messages for common errors
      if (errorMessage.includes('trim is not a function')) {
        errorMessage = 'There was a formatting issue with the message. Please try again.';
      } else if (errorMessage.includes('wallet is not connected')) {
        errorMessage = 'Your NEAR wallet is not connected. Please make sure you are logged in.';
      }
      
      statusElement.textContent = `Error: ${errorMessage}`;
      statusElement.className = 'error';
    }
  } finally {
    // Hide loading indicator regardless of outcome
    const loadingElement = document.getElementById('loading');
    if (loadingElement) {
      loadingElement.className = 'loading';
    }
  }
}

// Function to sign out
async function signOut() {
  if (typeof window !== 'undefined' && window.localStorage) {
    // Clear tokens from localStorage
    window.localStorage.removeItem('accessToken');
    window.localStorage.removeItem('refreshToken');
    window.localStorage.removeItem('tokenExpiry');
    window.localStorage.removeItem('clientId');
    
    // Update UI
    const statusElement = document.getElementById('auth-status');
    if (statusElement) {
      statusElement.textContent = '';
      statusElement.className = '';
    }
    
    // Hide account info
    const accountInfoElement = document.getElementById('account-info');
    const accountIdElement = document.getElementById('account-id');
    if (accountInfoElement && accountIdElement) {
      accountInfoElement.style.display = 'none';
      accountIdElement.textContent = 'Not signed in';
    }
    
    // Show sign in button, hide sign out button
    const signOutButton = document.getElementById('sign-out-button');
    const signInButton = document.getElementById('sign-in-button');
    if (signOutButton && signInButton) {
      signOutButton.style.display = 'none';
      signInButton.style.display = 'inline-block';
    }
    
    console.log('Signed out successfully');
  }
}

// Initialize the app when the page loads
document.addEventListener('DOMContentLoaded', () => {
  console.log('NEAR Auth frontend initialized');
  
  // Add click handler for sign in button
  const signInButton = document.getElementById('sign-in-button');
  if (signInButton) {
    signInButton.addEventListener('click', signWithNear);
  }
  
  // Add click handler for sign out button
  const signOutButton = document.getElementById('sign-out-button');
  if (signOutButton) {
    signOutButton.addEventListener('click', signOut);
  }
  
  // Check if user is already signed in
  if (typeof window !== 'undefined' && window.localStorage) {
    const accessToken = window.localStorage.getItem('accessToken');
    const tokenExpiry = window.localStorage.getItem('tokenExpiry');
    
    // If token exists and is not expired
    if (accessToken && tokenExpiry && parseInt(tokenExpiry) > Date.now()) {
      // Fetch user account info and update UI
      // This would typically involve a call to validate the token
      console.log('User already authenticated');
    }
  }
});
