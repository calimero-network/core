#!/usr/bin/env node

/**
 * WebSocket Authentication Test Script (Node.js)
 * Tests WebSocket authentication with a provided token
 */

const WebSocket = require('ws');
const fs = require('fs');
const path = require('path');

// Read package.json for config
const packageJsonPath = path.join(__dirname, 'package.json');
const packageJson = JSON.parse(fs.readFileSync(packageJsonPath, 'utf8'));

// Configuration - priority: command line > environment > package.json config
const WS_URL = process.env.WS_URL || process.env.npm_package_config_wsUrl || packageJson.config?.wsUrl || 'ws://localhost/ws';
const DEFAULT_TOKEN = process.env.npm_package_config_token || packageJson.config?.token;

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

function testWebSocket(url, token = null) {
  return new Promise((resolve, reject) => {
    const wsUrl = token ? `${url}?token=${encodeURIComponent(token)}` : url;
    log(`Connecting to: ${wsUrl}`, 'yellow');
    
    const ws = new WebSocket(wsUrl);
    
    const timeout = setTimeout(() => {
      ws.close();
      reject(new Error('WebSocket connection timeout'));
    }, 5000);
    
    ws.on('open', () => {
      clearTimeout(timeout);
      log('✅ WebSocket connection established', 'green');
      ws.close();
      resolve(true);
    });
    
    ws.on('error', (error) => {
      clearTimeout(timeout);
      reject(error);
    });
    
    ws.on('close', (code, reason) => {
      clearTimeout(timeout);
      if (code === 1000) {
        resolve(true);
      } else {
        reject(new Error(`WebSocket closed with code ${code}: ${reason}`));
      }
    });
  });
}

async function runTests() {
  log('🔐 Testing WebSocket Authentication', 'yellow');
  log('==================================', 'yellow');
  
  // Get token from command line argument, environment, or package.json config
  const token = process.argv[2] || process.env.TOKEN || DEFAULT_TOKEN;
  
  if (!token || token === 'your-jwt-token-here') {
    log('❌ Please provide a JWT token', 'red');
    log('Options:', 'yellow');
    log('1. Command line: npm test "your-jwt-token-here"', 'yellow');
    log('2. Environment: TOKEN=your-jwt-token-here npm test', 'yellow');
    log('3. Package config: Update "config.token" in package.json', 'yellow');
    process.exit(1);
  }
  
  log(`Using WebSocket URL: ${WS_URL}`, 'yellow');
  
  try {
    // Test 1: WebSocket connection with token
    log('\n1. Testing WebSocket connection with token...', 'yellow');
    await testWebSocket(WS_URL, token);
    log('✅ WebSocket connection with token successful', 'green');
    
    // Test 2: WebSocket connection without token (should fail)
    log('\n2. Testing WebSocket without token (should fail)...', 'yellow');
    try {
      await testWebSocket(WS_URL);
      log('❌ WebSocket connection succeeded without token (should have failed)', 'red');
      process.exit(1);
    } catch (error) {
      log('✅ WebSocket correctly rejected connection without token', 'green');
    }
    
    log('\n🎉 All tests completed successfully!', 'green');
    
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