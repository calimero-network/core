# Deploy merod on Phala Network (TEE)

This guide covers building, deploying, and configuring merod nodes on Phala Cloud's TEE infrastructure using dstack.

## Overview

When merod runs inside a Phala Confidential VM (CVM), it uses:

1. **mero-kms-phala** – Key Management Service that validates attestation and releases storage encryption keys
2. **dstack** – Phala's TEE runtime that provides attestation and key derivation via `tappd`
3. **TEE storage encryption** – Datastore and blobstore encrypted with keys derived from attestation

The node identity (libp2p keypair) is stored in `config.toml` and used to sign KMS challenge payloads. The storage encryption key is fetched from KMS at startup.

## Prerequisites

- [Phala Cloud account](https://cloud.phala.com/register)
- Docker Compose file for your application
- Understanding of [dstack](https://docs.phala.network/dstack/overview) and [Phala Cloud CVM](https://docs.phala.network/phala-cloud/cvm/create-with-docker-compose)

## Building the Images

### merod

Build from source:

```bash
cd core
cargo build --release -p merod
# Binary: target/release/merod
```

Or use the official Docker image (includes merod and meroctl):

```bash
docker pull ghcr.io/calimero-network/merod:<version>
```

### mero-kms-phala

The KMS service must run inside the same CVM as merod (or be reachable from it) to access the dstack socket.

Build from source:

```bash
cd core
cargo build --release -p mero-kms-phala
# Binary: target/release/mero-kms-phala
```

Or use the prebuilt container:

```bash
docker pull ghcr.io/calimero-network/mero-kms-phala:<version>
```

The KMS requires access to `/var/run/dstack.sock` (or `DSTACK_SOCKET_PATH`) to derive keys from the TEE attestation.

## Docker Compose for Phala CVM

All services in one compose file run inside the same CVM. Example:

```yaml
services:
  mero-kms:
    image: ghcr.io/calimero-network/mero-kms-phala:latest
    ports:
      - "8080:8080"
    environment:
      LISTEN_ADDR: "0.0.0.0:8080"
      DSTACK_SOCKET_PATH: "/var/run/dstack.sock"
      CHALLENGE_TTL_SECS: "60"
      ACCEPT_MOCK_ATTESTATION: "false"
      ENFORCE_MEASUREMENT_POLICY: "true"
      ALLOWED_TCB_STATUSES: "UpToDate"
      # Pin your image measurements (see "Pinning MRTD/RTMR" below)
      # ALLOWED_MRTD: "<hex>"
      # ALLOWED_RTMR0: "<hex>"
      # ALLOWED_RTMR1: "<hex>"
      # ALLOWED_RTMR2: "<hex>"
      # ALLOWED_RTMR3: "<hex>"
    volumes:
      - /var/run/dstack.sock:/var/run/dstack.sock

  merod:
    image: ghcr.io/calimero-network/merod:latest
    ports:
      - "2428:2428"   # RPC
      - "2528:2528"  # P2P swarm
    environment:
      CALIMERO_HOME: "/data"
    volumes:
      - merod-data:/data
    depends_on:
      - mero-kms
```

**Important:** `mero-kms` must start before `merod` so the KMS is available when merod fetches its storage key at startup.

## Deploying to Phala Cloud

1. **Create a Phala Cloud account** at [cloud.phala.com](https://cloud.phala.com/register).

2. **Prepare your Docker Compose** with merod and mero-kms-phala as above.

3. **Deploy via Phala Cloud UI:**
   - Go to the deployment section
   - Switch to the Advanced tab
   - Paste or upload your `docker-compose.yml`
   - Deploy

4. **Or use Phala Cloud CLI** (see [Start from Cloud CLI](https://docs.phala.network/phala-cloud/phala-cloud-cli/start-from-cloud-cli)).

5. **Verify attestation** – Phala provides RA (Remote Attestation) reports. Use the [TEE Attestation Explorer](https://ra-quote-explorer.vercel.app/) to verify your CVM is running in a TEE.

## Setting Up merod for TEE

### 1. Initialize the node

```bash
merod --home /data --node default init \
  --server-port 2428 \
  --swarm-port 2528 \
  --boot-network calimero-dev
```

For read-only TEE nodes:

```bash
merod --home /data --node default init \
  --mode read-only \
  --server-port 2428 \
  --swarm-port 2528 \
  --boot-network calimero-dev
```

### 2. Add TEE/KMS configuration

Add the Phala KMS URL to `config.toml`. The KMS must be reachable from merod (e.g. `http://mero-kms:8080/` when both run in the same CVM).

```bash
merod --home /data --node default config \
  'tee.kms.phala.url="http://mero-kms:8080/"'
```

Or edit `config.toml` directly:

```toml
[tee]
[tee.kms.phala]
url = "http://mero-kms:8080/"
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

## Pinning MRTD/RTMR (Production)

For production, pin the measurements of your deployed image so only that exact software stack can obtain keys.

1. **Deploy your CVM** (e.g. with `ACCEPT_MOCK_ATTESTATION=true` or relaxed policy initially).

2. **Obtain measurements** from your running CVM:
   - Use Phala's RA report / attestation tools
   - Or build a reproducible image and extract MRTD/RTMR from the build pipeline

3. **Configure the KMS** with the trusted values:

   ```bash
   export ENFORCE_MEASUREMENT_POLICY=true
   export ACCEPT_MOCK_ATTESTATION=false
   export ALLOWED_TCB_STATUSES=UpToDate
   export ALLOWED_MRTD=<your_mrtd_hex>
   export ALLOWED_RTMR0=<your_rtmr0_hex>
   export ALLOWED_RTMR1=<your_rtmr1_hex>
   export ALLOWED_RTMR2=<your_rtmr2_hex>
   export ALLOWED_RTMR3=<your_rtmr3_hex>
   ```

4. **Restart the KMS** with the new policy. Only nodes whose attestation matches these values will receive keys.

See [mero-kms-phala README](../crates/mero-kms-phala/README.md) for full KMS configuration.

## Development Mode (Non-TEE)

For local testing without TDX hardware:

1. Run mero-kms-phala with `ACCEPT_MOCK_ATTESTATION=true`
2. merod will use mock attestation (if available in the build)
3. The KMS will accept mock quotes and release keys

**Do not use mock attestation in production.**

## Troubleshooting

### merod fails to fetch key from KMS

- Ensure the KMS URL in `config.toml` is correct and reachable from merod
- Check that mero-kms has access to `/var/run/dstack.sock`
- Verify the KMS is running and `/health` returns 200

### KMS rejects attestation

- If using mock attestation, set `ACCEPT_MOCK_ATTESTATION=true` (dev only)
- In production, ensure MRTD/RTMR are pinned and match your image
- Check `ALLOWED_TCB_STATUSES` (e.g. `UpToDate`)

### Node identity in config.toml

The node identity (libp2p keypair) is stored in `config.toml`. It is required for:
- Signing KMS challenge payloads
- P2P networking (peer ID)

Keep `config.toml` backed up; losing it means losing the node identity.

## See Also

- [merod README](../crates/merod/README.md) – TEE storage encryption and KMS flow
- [mero-kms-phala README](../crates/mero-kms-phala/README.md) – KMS endpoints and policy
- [Phala Cloud Documentation](https://docs.phala.network/)
- [dstack Overview](https://docs.phala.network/dstack/overview)
