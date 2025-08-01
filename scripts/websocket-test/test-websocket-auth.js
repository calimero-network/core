#!/usr/bin/env node

/**
 * WebSocket Authentication Test Script (Node.js)
 * Tests WebSocket authentication with a provided token
 */

const WebSocket = require('ws');

// Get token from command line argument, environment, or package.json config
const token = process.argv[2] || process.env.TOKEN || (process.env.npm_config_token || 'your-jwt-token-here');

const url = `ws://localhost:80/ws`;

console.log('🔐 Testing WebSocket Authentication');
console.log('==================================');

if (!token || token === 'your-jwt-token-here') {
    console.error('❌ Error: No token provided. Please provide a token as a command line argument, via TOKEN environment variable, or in package.json config.');
    process.exit(1);
}

function testWebSocket(url, token, withToken) {
    const fullUrl = withToken ? `${url}?token=${token}` : url;
    console.log(`\nConnecting to: ${fullUrl}`);
    const ws = new WebSocket(fullUrl);

    ws.on('open', function open() {
        console.log(`✅ Test passed: WebSocket connection opened successfully.`);
        ws.close();
    });

    ws.on('error', function error(err) {
        if (withToken) {
            console.error(`❌ Test failed: ${err.message}`);
        } else {
            if (err.message.includes('401')) {
                console.log(`✅ Test passed: Received expected 401 Unauthorized without token.`);
            } else {
                console.error(`❌ Test failed with unexpected error: ${err.message}`);
            }
        }
    });
}

// Test with the provided token
testWebSocket(url, token, true);

// Test without any token
testWebSocket(url, token, false); 