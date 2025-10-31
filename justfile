# justfile - Developer commands for Calimero

default: build

# Build everything
build:
	cargo build --workspace --all-features

# Desktop mode (no HTTP server)
desktop:
	cargo build -p calimero-server --no-default-features

# Server mode (with HTTP)
server:
	cargo build -p calimero-server --features http-server

# Run all quality checks
check:
	cargo clippy --workspace --all-targets --all-features -- -D warnings
	cargo test --workspace --all-features

# Security audit
audit:
	cargo audit
	cargo deny check

# Verify no OpenSSL in dependency tree
verify-tls:
	@echo "Checking for OpenSSL/native-tls..."
	@cargo tree -p calimero-node --all-features | grep -Ei 'openssl|native-tls' && \
		(echo "❌ Found native TLS"; exit 1) || \
		echo "✅ Pure rustls stack"

# Clean everything
clean:
	cargo clean
	rm -rf target

# Format all code
fmt:
	cargo fmt --all

# Watch mode for tests
watch-test:
	cargo watch -x "test --workspace"

# Run specific test
test *args:
	cargo test --workspace {{args}}

# Update dependencies
update:
	cargo update

# Check for outdated dependencies
outdated:
	cargo outdated --workspace

