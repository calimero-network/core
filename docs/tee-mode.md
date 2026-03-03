# merod in TEE Mode

This document describes how **merod** operates in TEE (Trusted Execution Environment) mode. merod runs inside a confidential VM and obtains its storage encryption key from a KMS at startup. Actual deployment (GCP, Phala, etc.) is documented in [mero-tee](https://github.com/calimero-network/mero-tee).

## Overview

When merod runs in TEE mode:

1. **merod** – At startup, requests a storage key from a KMS using attestation and node identity.
2. **KMS** – Validates attestation and releases the key. Must run in the same TEE environment (or be reachable) to access attestation services.
3. **TEE runtime** – Provides attestation and key derivation (e.g. TDX on Intel, dstack on Phala).

The node identity (libp2p keypair) is stored in `config.toml` and used to sign KMS challenge payloads.

## KMS Providers

merod currently supports:

- **Phala Cloud** – `mero-kms-phala` from [mero-tee](https://github.com/calimero-network/mero-tee). Used when merod runs in a Phala CVM with dstack.

For deployment, KMS build, and configuration, see [mero-tee](https://github.com/calimero-network/mero-tee):

- **[Deploy on GCP](https://github.com/calimero-network/mero-tee/blob/master/docs/deploy-gcp.md)** – TDX locked images, Packer build
- **[Deploy on Phala](https://github.com/calimero-network/mero-tee/blob/master/docs/deploy-phala.md)** – Phala CVM with mero-kms-phala
- **[mero-tee releases](https://github.com/calimero-network/mero-tee/releases)** – mero-kms-phala binaries, MRTDs, attestation artifacts

## Building merod

From this repository:

```bash
cd core
cargo build --release -p merod
# Binary: target/release/merod
```

Or use the official release binaries from [core releases](https://github.com/calimero-network/core/releases).

## Configuring merod for TEE

### 1. Initialize the node

```bash
merod --home /data --node default init \
  --server-port 2428 \
  --swarm-port 2528 \
  --boot-network calimero-dev
```

For read-only nodes:

```bash
merod --home /data --node default init \
  --mode read-only \
  --server-port 2428 \
  --swarm-port 2528 \
  --boot-network calimero-dev
```

### 2. Add TEE/KMS configuration

Configure the KMS URL. It must be reachable from merod.

For Phala KMS:

```bash
merod --home /data --node default config \
  'tee.kms.phala.url="http://<kms-host>:8080/"'
```

Or edit `config.toml` directly:

```toml
[tee]
[tee.kms.phala]
url = "http://<kms-host>:8080/"
```

### 3. Run merod

```bash
merod --home /data --node default run
```

On startup, merod will:

1. Request a challenge from the KMS
2. Generate a TDX attestation quote with the challenge nonce and peer ID hash
3. Sign the payload with the node identity key
4. Submit the signed request to the KMS
5. Receive the storage encryption key and use it for the datastore/blobstore

## Troubleshooting

### merod fails to fetch key from KMS

- Ensure `tee.kms.phala.url` in `config.toml` is correct and reachable from merod
- Verify the KMS is running and `/health` returns 200
- Check that the KMS has access to the TEE attestation socket (e.g. dstack on Phala)

### KMS rejects attestation

- For development: configure KMS to accept mock attestation (see mero-tee docs)
- For production: ensure measurement policy is correctly configured (see [mero-tee mero-kms-phala](https://github.com/calimero-network/mero-tee/blob/master/crates/mero-kms-phala/README.md))

### Node identity in config.toml

The node identity (libp2p keypair) is stored in `config.toml`. It is required for:
- Signing KMS challenge payloads
- P2P networking (peer ID)

Keep `config.toml` backed up; losing it means losing the node identity.

## GCP operators: MRTD verification

For GCP TDX nodes, operators verify deployed nodes against published measurements. Fetch `published-mrtds.json` from mero-tee releases:

```
https://github.com/calimero-network/mero-tee/releases/download/<X.Y.Z>/published-mrtds.json
```

Example: `https://github.com/calimero-network/mero-tee/releases/download/2.1.1/published-mrtds.json`

## See Also

- [merod README](../crates/merod/README.md) – TEE storage encryption and KMS flow
- [mero-tee](https://github.com/calimero-network/mero-tee) – Deployment (GCP, Phala), KMS, locked images
- [mero-tee deploy-gcp](https://github.com/calimero-network/mero-tee/blob/master/docs/deploy-gcp.md) – GCP TDX locked images
- [mero-tee deploy-phala](https://github.com/calimero-network/mero-tee/blob/master/docs/deploy-phala.md) – Phala CVM deployment
- [mero-kms-phala README](https://github.com/calimero-network/mero-tee/blob/master/crates/mero-kms-phala/README.md) – KMS build, deployment, and policy
