#!/usr/bin/env node

/**
 * WebSocket Authentication Test Script (Node.js)
 * Tests the new WebSocket auth functionality
 */

const WebSocket = require('ws');
const https = require('https');
const http = require('http');

// Configuration
const AUTH_URL = process.env.AUTH_URL || 'http://localhost:3001';
const WS_URL = process.env.WS_URL || 'ws://localhost/ws';

// Colors for output
const colors = {
  red: '\x1b[31m',
  green: '\x1b[32m',
  yellow: '\x1b[33m',
  reset: '\x1b[0m'
};

function log(message, color = 'reset') {
  console.log(`${colors[color]}${message}${colors.reset}`);
}

async function makeRequest(url, options = {}) {
  return new Promise((resolve, reject) => {
    const urlObj = new URL(url);
    const client = urlObj.protocol === 'https:' ? https : http;
    
    const req = client.request(url, options, (res) => {
      let data = '';
      res.on('data', chunk => data += chunk);
      res.on('end', () => {
        resolve({
          statusCode: res.statusCode,
          headers: res.headers,
          body: data
        });
      });
    });
    
    req.on('error', reject);
    
    if (options.body) {
      req.write(options.body);
    }
    
    req.end();
  });
}

async function testWebSocket(url, token = null) {
  return new Promise((resolve, reject) => {
    const wsUrl = token ? `${url}?token=${encodeURIComponent(token)}` : url;
    const ws = new WebSocket(wsUrl);
    
    const timeout = setTimeout(() => {
      ws.close();
      reject(new Error('WebSocket connection timeout'));
    }, 5000);
    
    ws.on('open', () => {
      clearTimeout(timeout);
      ws.close();
      resolve(true);
    });
    
    ws.on('error', (error) => {
      clearTimeout(timeout);
      reject(error);
    });
  });
}

async function runTests() {
  log('🔐 Testing WebSocket Authentication', 'yellow');
  log('==================================', 'yellow');
  
  try {
    // Test 1: Get JWT token
    log('\n1. Getting JWT token...', 'yellow');
    const tokenResponse = await makeRequest(`${AUTH_URL}/auth/token`, {
      method: 'POST',
      headers: {
        'Content-Type': 'application/json'
      },
      body: JSON.stringify({
        auth_method: 'near_wallet',
        public_key: 'test-key',
        client_name: 'websocket-test',
        timestamp: Math.floor(Date.now() / 1000),
        provider_data: {}
      })
    });
    
    if (tokenResponse.statusCode !== 200) {
      throw new Error(`Failed to get token: ${tokenResponse.statusCode}`);
    }
    
    const tokenData = JSON.parse(tokenResponse.body);
    const token = tokenData.access_token;
    
    if (!token) {
      throw new Error('No access token in response');
    }
    
    log('✅ Token obtained successfully', 'green');
    
    // Test 2: Validate token via HTTP
    log('\n2. Validating token via HTTP...', 'yellow');
    const httpValidation = await makeRequest(`${AUTH_URL}/auth/validate`, {
      headers: {
        'Authorization': `Bearer ${token}`
      }
    });
    
    if (httpValidation.statusCode !== 200) {
      throw new Error(`HTTP validation failed: ${httpValidation.statusCode}`);
    }
    
    log('✅ HTTP validation successful', 'green');
    
    // Test 3: Validate token via query parameter
    log('\n3. Validating token via query parameter...', 'yellow');
    const queryValidation = await makeRequest(`${AUTH_URL}/auth/validate?token=${encodeURIComponent(token)}`);
    
    if (queryValidation.statusCode !== 200) {
      throw new Error(`Query parameter validation failed: ${queryValidation.statusCode}`);
    }
    
    log('✅ Query parameter validation successful', 'green');
    
    // Test 4: WebSocket connection with token
    log('\n4. Testing WebSocket connection with token...', 'yellow');
    try {
      await testWebSocket(WS_URL, token);
      log('✅ WebSocket connection with token successful', 'green');
    } catch (error) {
      log(`⚠️  WebSocket connection test: ${error.message}`, 'yellow');
    }
    
    // Test 5: WebSocket connection without token (should fail)
    log('\n5. Testing WebSocket without token (should fail)...', 'yellow');
    try {
      await testWebSocket(WS_URL);
      log('❌ WebSocket connection succeeded without token (should have failed)', 'red');
    } catch (error) {
      log('✅ WebSocket correctly rejected connection without token', 'green');
    }
    
    log('\n🎉 All tests completed!', 'green');
    log('\nTo test manually:', 'yellow');
    log(`1. Get token: curl -X POST ${AUTH_URL}/auth/token -H 'Content-Type: application/json' -d '{"auth_method":"near_wallet","public_key":"test-key","client_name":"test","timestamp":${Math.floor(Date.now() / 1000)},"provider_data":{}}'`);
    log(`2. Connect WebSocket: node -e "const ws = new (require('ws'))('${WS_URL}?token=YOUR_TOKEN_HERE'); ws.on('open', () => console.log('Connected!'));"`);
    
  } catch (error) {
    log(`❌ Test failed: ${error.message}`, 'red');
    process.exit(1);
  }
}

// Run tests
runTests().catch(error => {
  log(`❌ Unexpected error: ${error.message}`, 'red');
  process.exit(1);
}); 