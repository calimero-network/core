# IDENTITY

## DID:WEB
```
{
  "did": "did:web:vuki",
  "controllerKeyId": "04522e4c259416682b86403ac3c049b43cdf53305b3337e29fe5b31d7a3bee7cb3c3b5ffb52f957e86aafa4373e9e08a87bd40c9c70403119bb8918b0974ec74da",
  "provider": "did:web",
  "services": [],
  "keys": [
    {
      "kid": "04522e4c259416682b86403ac3c049b43cdf53305b3337e29fe5b31d7a3bee7cb3c3b5ffb52f957e86aafa4373e9e08a87bd40c9c70403119bb8918b0974ec74da",
      "type": "Secp256k1",
      "kms": "local",
      "publicKeyHex": "04522e4c259416682b86403ac3c049b43cdf53305b3337e29fe5b31d7a3bee7cb3c3b5ffb52f957e86aafa4373e9e08a87bd40c9c70403119bb8918b0974ec74da",
      "meta": {
        "algorithms": [
          "ES256K",
          "ES256K-R",
          "eth_signTransaction",
          "eth_signTypedData",
          "eth_signMessage",
          "eth_rawSign"
        ]
      }
    }
  ],
  "alias": "vuki"
  }
  ```

  ## DID:CALI
```
{
  "did": "did:cali:vuki2",
  "controllerKeyId": "048b66d6adad6354cc2380df1cea7885aa805da6dca18b0162ce5965d83b3669536fb51204d1ab63c93a62e61296595ee5cb7218b388894d991c18b95f7b412c03",
  "keys": [
    {
      "type": "Secp256k1",
      "kid": "048b66d6adad6354cc2380df1cea7885aa805da6dca18b0162ce5965d83b3669536fb51204d1ab63c93a62e61296595ee5cb7218b388894d991c18b95f7b412c03",
      "publicKeyHex": "048b66d6adad6354cc2380df1cea7885aa805da6dca18b0162ce5965d83b3669536fb51204d1ab63c93a62e61296595ee5cb7218b388894d991c18b95f7b412c03",
      "meta": {
        "algorithms": [
          "ES256K",
          "ES256K-R",
          "eth_signTransaction",
          "eth_signTypedData",
          "eth_signMessage",
          "eth_rawSign"
        ]
      },
      "kms": "local"
    }
  ],
  "services": [],
  "provider": "did:cali",
  "alias": "vuki2"
}
```

  # CREDENTIALS
  ```
{
  "credentialSubject": {
    "you": "vuki.near",
    "id": "vuki"
  },
  "issuer": {
    "id": "did:web:vuki"
  },
  "type": [
    "VerifiableCredential"
  ],
  "@context": [
    "https://www.w3.org/2018/credentials/v1"
  ],
  "issuanceDate": "2024-02-02T15:26:55.000Z",
  "proof": {
    "type": "JwtProof2020",
    "jwt": "eyJhbGciOiJFUzI1NksiLCJ0eXAiOiJKV1QifQ.eyJ2YyI6eyJAY29udGV4dCI6WyJodHRwczovL3d3dy53My5vcmcvMjAxOC9jcmVkZW50aWFscy92MSJdLCJ0eXBlIjpbIlZlcmlmaWFibGVDcmVkZW50aWFsIl0sImNyZWRlbnRpYWxTdWJqZWN0Ijp7InlvdSI6InZ1a2kubmVhciJ9fSwic3ViIjoidnVraSIsIm5iZiI6MTcwNjg4NzYxNSwiaXNzIjoiZGlkOndlYjp2dWtpIn0.1DmzUW_QAKzEF6GLmNut2xogqj8nF58pSeXi86zX-i92EIWyjPitRPonq5QV95BY1V9UVeOihRGYlnHqnQGbdg"
  }
}
```
