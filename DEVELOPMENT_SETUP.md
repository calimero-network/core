# 🚀 Development Setup Guide

This guide covers the development environment setup for the Calimero Core project, including pre-commit hooks and code quality tools.

## 📋 **Prerequisites**

### **Required Tools:**
- **Rust**: Latest stable version (1.70+)
- **Node.js**: Version 16+ (for Husky pre-commit hooks)
- **Git**: Latest version
- **Docker**: For containerized development

### **Optional Tools:**
- **cargo-watch**: For development hot-reloading
- **cargo-audit**: For security vulnerability scanning

---

## 🔧 **Initial Setup**

### **1. Clone and Setup Repository**
```bash
git clone https://github.com/calimero-network/core.git
cd core
```

### **2. Install Rust Dependencies**
```bash
# Install Rust (if not already installed)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Install required components
rustup component add rustfmt clippy

# Install cargo-watch for development (optional)
cargo install cargo-watch
```

### **3. Setup Pre-commit Hooks**
```bash
# Run the setup script
./scripts/setup-husky.sh

# Or manually:
npm install
npx husky install
chmod +x .husky/pre-commit
```

---

## 🎯 **Pre-commit Hooks**

### **What Runs on Commit:**
- ✅ **Rust Formatting**: `cargo fmt --all -- --check`
- ✅ **Rust Linting**: `cargo clippy --all-targets --all-features -- -D warnings`
- ✅ **TOML Validation**: Syntax checking for Cargo.toml files
- ✅ **YAML Validation**: Syntax checking for workflow files

### **Manual Commands:**
```bash
# Format Rust code
npm run format:rust
# or
cargo fmt --all

# Lint Rust code
npm run lint:rust
# or
cargo clippy --all-targets --all-features -- -D warnings

# Check Rust code
npm run check:rust
# or
cargo check --all-targets --all-features

# Run tests
npm run test:rust
# or
cargo test --all-targets --all-features
```

---

## 🔍 **Code Quality Tools**

### **Rustfmt Configuration**
The project uses a custom `rustfmt.toml` configuration:
```toml
group_imports = "StdExternalCrate"
imports_granularity = "Module"
```

### **Clippy Configuration**
Clippy runs with strict warnings enabled:
- `-D warnings`: Treats all warnings as errors
- `--all-targets`: Checks all targets (lib, bin, tests, examples)
- `--all-features`: Enables all features for comprehensive checking

### **Cargo Configuration**
- **Workspace**: Multi-crate workspace with shared dependencies
- **Edition**: 2021
- **Resolver**: Version 2

---

## 🧪 **Development Workflow**

### **1. Making Changes**
```bash
# Create a new branch
git checkout -b feature/your-feature-name

# Make your changes
# ... edit files ...

# Stage changes
git add .

# Pre-commit hooks will run automatically
git commit -m "feat: your feature description"
```

### **2. If Pre-commit Fails**
```bash
# Fix formatting issues
cargo fmt --all

# Fix linting issues
cargo clippy --all-targets --all-features -- -D warnings

# Re-commit
git add .
git commit -m "feat: your feature description"
```

### **3. Testing Your Changes**
```bash
# Run all tests
cargo test --all-targets --all-features

# Run specific crate tests
cargo test -p calimero-node

# Run with verbose output
cargo test --all-targets --all-features -- --nocapture
```

---

## 🐳 **Docker Development**

### **Build Docker Image**
```bash
# Build the development image
docker build -t calimero-core:dev .

# Or use the rebuild script
./rebuild-image.sh
```

### **Run with Docker Compose**
```bash
# Start the development environment
docker-compose -f docker-compose.nodes.yml up -d

# View logs
docker-compose -f docker-compose.nodes.yml logs -f
```

---

## 📊 **Performance Testing**

### **Run Performance Tests**
```bash
# Quick performance test
merobox bootstrap run workflows/bootstrap-short.yml

# Comprehensive performance test
merobox bootstrap run workflows/bootstrap.yml

# Phase 2 optimization test
merobox bootstrap run workflows/phase2-performance-test.yml

# Phase 3 optimization test
merobox bootstrap run workflows/phase3-performance-test.yml
```

### **CRDT Convergence Test**
```bash
# Test CRDT convergence
merobox bootstrap run workflows/convergence-test.yml
```

---

## 🔧 **Troubleshooting**

### **Common Issues:**

#### **1. Pre-commit Hook Fails**
```bash
# Check if Husky is properly installed
ls -la .husky/pre-commit

# Reinstall Husky
npm install
npx husky install

# Make hook executable
chmod +x .husky/pre-commit
```

#### **2. Rust Formatting Issues**
```bash
# Format all code
cargo fmt --all

# Check specific file
cargo fmt --check src/main.rs
```

#### **3. Clippy Issues**
```bash
# Run clippy with specific crate
cargo clippy -p calimero-node

# Allow specific warnings (if needed)
# Add #[allow(clippy::specific_warning)] to your code
```

#### **4. Build Issues**
```bash
# Clean and rebuild
cargo clean
cargo build --all-targets --all-features

# Check for missing dependencies
cargo check --all-targets --all-features
```

---

## 📈 **Code Quality Metrics**

### **Current Standards:**
- ✅ **Zero Clippy Warnings**: All code must pass clippy with `-D warnings`
- ✅ **Consistent Formatting**: All code formatted with rustfmt
- ✅ **Valid TOML/YAML**: All configuration files must be syntactically valid
- ✅ **Passing Tests**: All tests must pass before merging

### **Performance Targets:**
- 🎯 **Propagation Time**: 0s (immediate)
- 🎯 **Convergence Time**: <200ms
- 🎯 **Scale**: 8+ nodes stable
- 🎯 **Reliability**: 100% convergence

---

## 🚀 **CI/CD Integration**

### **GitHub Actions**
The project uses GitHub Actions for:
- ✅ Automated testing
- ✅ Code quality checks
- ✅ Performance benchmarking
- ✅ Security scanning

### **Pre-commit Integration**
All commits are automatically checked for:
- ✅ Code formatting
- ✅ Linting issues
- ✅ Syntax validation
- ✅ Test coverage

---

## 📚 **Additional Resources**

### **Rust Documentation:**
- [Rust Book](https://doc.rust-lang.org/book/)
- [Rust Reference](https://doc.rust-lang.org/reference/)
- [Cargo Book](https://doc.rust-lang.org/cargo/)

### **Project Documentation:**
- [README.md](README.md) - Project overview
- [STYLE.md](STYLE.md) - Coding style guidelines
- [CONTRIBUTING.md](CONTRIBUTING.md) - Contribution guidelines

### **Performance Documentation:**
- [PERFORMANCE_OPTIMIZATION_SUMMARY.md](PERFORMANCE_OPTIMIZATION_SUMMARY.md) - Performance improvements
- [REFACTORING_SUMMARY.md](REFACTORING_SUMMARY.md) - Code refactoring details

---

*Last Updated: Performance Optimization Branch*  
*Status: ✅ Complete Development Setup*
