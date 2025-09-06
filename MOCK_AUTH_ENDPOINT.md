# Mock Authentication Endpoint

This document describes the mock authentication endpoint designed for CI/testing purposes.

## ⚠️ Security Warning

**The mock authentication endpoint BYPASSES ALL AUTHENTICATION and should NEVER be enabled in production environments.**

## Overview

The mock endpoint (`/auth/mock-token`) allows you to generate valid JWT tokens without going through the normal authentication flow. This is useful for:

- **Continuous Integration (CI)** testing
- **Local development** and testing
- **Automated testing** scenarios
- **Terminal/CLI tools** that need quick token generation

## Configuration

### Enabling the Endpoint

Add the following to your auth service configuration:

```toml
[development]
# Enable the mock token endpoint (disabled by default)
enable_mock_auth = true

# Require authorization header for additional security (recommended)
mock_auth_require_header = true

# Optional: Specific authorization header value required
mock_auth_header_value = "Bearer test-auth-token"
```

### Configuration Options

| Option | Type | Default | Description |
|--------|------|---------|-------------|
| `enable_mock_auth` | boolean | `false` | Enable/disable the mock endpoint |
| `mock_auth_require_header` | boolean | `true` | Require Authorization header |
| `mock_auth_header_value` | string | `null` | Specific header value to require |

## API Usage

### Endpoint

```
POST /auth/mock-token
```

### Headers

```
Content-Type: application/json
Authorization: Bearer test-auth-token  # If required by config
```

### Request Body

```json
{
  "client_name": "my-test-client",
  "permissions": ["admin"],           // Optional, defaults to ["admin"]
  "node_url": "http://localhost:3000", // Optional
  "access_token_expiry": 3600,        // Optional, uses config default
  "refresh_token_expiry": 86400       // Optional, uses config default
}
```

### Response

```json
{
  "data": {
    "access_token": "eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9...",
    "refresh_token": "eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9...",
    "error": null
  },
  "error": null
}
```

### Response Headers

The endpoint includes warning headers:

```
X-Mock-Token: true
X-Key-Id: mock_my-test-client_1698765432
X-Warning: Mock token - for testing only
```

## Usage Examples

### cURL

```bash
curl -X POST http://localhost:3001/auth/mock-token \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer test-auth-token" \
  -d '{
    "client_name": "ci-test-client",
    "permissions": ["admin", "read"]
  }'
```

### Terminal/CLI Usage

```bash
# Get tokens for testing
TOKEN_RESPONSE=$(curl -s -X POST http://localhost:3001/auth/mock-token \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer test-auth-token" \
  -d '{"client_name": "terminal-test"}')

ACCESS_TOKEN=$(echo $TOKEN_RESPONSE | jq -r '.data.access_token')

# Use the token for authenticated requests
curl -H "Authorization: Bearer $ACCESS_TOKEN" \
  http://localhost:3001/admin/keys
```

### CI/CD Integration

```yaml
# GitHub Actions example
- name: Get mock auth token
  run: |
    RESPONSE=$(curl -s -X POST ${{ secrets.AUTH_URL }}/auth/mock-token \
      -H "Content-Type: application/json" \
      -H "Authorization: Bearer ${{ secrets.MOCK_AUTH_TOKEN }}" \
      -d '{"client_name": "ci-${{ github.run_id }}"}')
    
    echo "AUTH_TOKEN=$(echo $RESPONSE | jq -r '.data.access_token')" >> $GITHUB_ENV

- name: Run authenticated tests
  run: |
    curl -H "Authorization: Bearer $AUTH_TOKEN" \
      ${{ secrets.AUTH_URL }}/admin/identity
```

## Security Considerations

1. **Production Safety**: The endpoint returns 404 when `enable_mock_auth` is false
2. **Authorization Header**: Require a secret header value for additional protection
3. **Audit Logging**: All mock token generation is logged with warnings
4. **Temporary Keys**: Generated keys are prefixed with "mock_" for identification
5. **Clear Headers**: Response includes warning headers to identify mock tokens

## Token Properties

Mock tokens have the following characteristics:

- **Valid JWT**: Properly signed tokens that work with all validation
- **Configurable Expiry**: Use default or custom expiration times
- **Full Permissions**: Can include any permission set needed for testing
- **Node-Specific**: Support for node-specific token validation
- **Traceable**: Key IDs include "mock_" prefix and timestamp

## Troubleshooting

### Endpoint Not Found (404)

- Check that `enable_mock_auth = true` in configuration
- Verify you're using the correct endpoint path `/auth/mock-token`

### Unauthorized (401)

- Ensure Authorization header is provided if `mock_auth_require_header = true`
- Check that header value matches `mock_auth_header_value` if specified

### Bad Request (400)

- Verify `client_name` is provided and not empty
- Check JSON payload format

## Best Practices

1. **Environment Isolation**: Only enable in test/dev environments
2. **Secret Management**: Store authorization tokens securely
3. **Cleanup**: Consider implementing token cleanup for long-running tests
4. **Monitoring**: Watch for unexpected mock token usage in logs
5. **Documentation**: Clearly document when mock tokens are used in tests

## Integration with Existing Systems

The mock endpoint generates real JWT tokens that work with:

- All existing token validation middleware
- Permission checking systems
- Token refresh flows (though refresh will use real authentication)
- Node-specific validation

This ensures your tests accurately reflect production behavior while bypassing the authentication step.
