# Publishing Guide for Calimero Client Python Bindings

This guide explains how to publish the `calimero-client-py-bindings` package to PyPI and other Python package repositories.

## 🚀 Prerequisites

### 1. PyPI Account
- Create an account on [PyPI](https://pypi.org/account/register/)
- Enable two-factor authentication (2FA) for security
- Create an API token for automated publishing

### 2. Test PyPI Account (Recommended)
- Create an account on [Test PyPI](https://test.pypi.org/account/register/)
- Use this for testing before publishing to production

### 3. Required Tools
```bash
# Install publishing tools
pip install twine build

# Install maturin for Rust bindings
pip install maturin
```

## 📦 Building the Package

### 1. Clean Build Environment
```bash
# Remove previous builds
rm -rf target/wheels/
rm -rf dist/
rm -rf build/
rm -rf *.egg-info/
```

### 2. Build with Maturin
```bash
# Build the Python wheel
maturin build --features python --release

# Verify the wheel was created
ls -la target/wheels/
```

### 3. Build Source Distribution (Optional)
```bash
# Build source distribution
python -m build --sdist

# Verify source distribution
ls -la dist/
```

## 🧪 Testing Before Publishing

### 1. Test Installation
```bash
# Install from wheel
pip install target/wheels/calimero_client_py_bindings-*.whl

# Test import
python -c "import calimero_client_py_bindings; print('Import successful')"

# Test CLI
calimero-client-py --help
```

### 2. Test on Test PyPI
```bash
# Upload to Test PyPI
twine upload --repository testpypi target/wheels/*.whl

# Install from Test PyPI
pip install --index-url https://test.pypi.org/simple/ calimero-client-py-bindings

# Test functionality
python -c "import calimero_client_py_bindings; print('Test PyPI install successful')"
```

## 📤 Publishing to PyPI

### 1. Final Verification
```bash
# Check package metadata
twine check target/wheels/*.whl

# Verify package contents
pip show calimero-client-py-bindings
```

### 2. Upload to PyPI
```bash
# Upload to production PyPI
twine upload target/wheels/*.whl

# Or upload both wheel and source
twine upload dist/*
```

### 3. Verify Publication
```bash
# Wait a few minutes for PyPI to update
# Check on https://pypi.org/project/calimero-client-py-bindings/

# Test installation from PyPI
pip install calimero-client-py-bindings

# Verify it works
python -c "import calimero_client_py_bindings; print('PyPI install successful')"
```

## 🔄 Automated Publishing with GitHub Actions

The package includes a GitHub Actions workflow that automatically publishes when you create a release tag.

### 1. Create a Release
```bash
# Tag the release
git tag -a v0.1.0 -m "Release version 0.1.0"

# Push the tag
git push origin v0.1.0
```

### 2. Set Up Secrets
In your GitHub repository settings, add these secrets:
- `PYPI_API_TOKEN`: Your PyPI API token
- `TEST_PYPI_API_TOKEN`: Your Test PyPI API token (optional)

### 3. Monitor the Workflow
- Check the Actions tab in GitHub
- The workflow will automatically:
  - Build the package
  - Run tests
  - Publish to PyPI

## 📋 Pre-Publishing Checklist

### Code Quality
- [ ] All tests pass (`pytest`)
- [ ] Code is formatted (`black src/`)
- [ ] Imports are sorted (`isort src/`)
- [ ] Type checking passes (`mypy src/`)
- [ ] Linting passes (`flake8 src/`)

### Documentation
- [ ] README.md is up to date
- [ ] API documentation is current
- [ ] Examples are working
- [ ] Changelog is updated

### Package Configuration
- [ ] Version number is correct in all files
- [ ] Dependencies are properly specified
- [ ] Package metadata is accurate
- [ ] License information is correct

### Testing
- [ ] Package builds successfully
- [ ] Installation works
- [ ] Basic functionality tested
- [ ] CLI commands work
- [ ] Import statements succeed

## 🚨 Common Issues and Solutions

### 1. Build Failures
```bash
# Check Rust toolchain
rustup show

# Update dependencies
cargo update

# Clean and rebuild
cargo clean
maturin build --features python
```

### 2. Import Errors
```bash
# Verify module structure
python -c "import sys; print(sys.path)"

# Check package installation
pip list | grep calimero
```

### 3. PyPI Upload Errors
```bash
# Check authentication
twine check target/wheels/*.whl

# Verify API token
echo $PYPI_API_TOKEN

# Test with Test PyPI first
twine upload --repository testpypi target/wheels/*.whl
```

## 🔧 Maintenance

### 1. Version Management
```bash
# Update version in all files:
# - pyproject.toml
# - setup.py
# - src/calimero_client_py_bindings/__init__.py
# - README.md (if version is mentioned)
```

### 2. Dependency Updates
```bash
# Update Rust dependencies
cargo update

# Update Python dependencies
pip install --upgrade -r requirements-dev.txt

# Test with updated dependencies
pytest
```

### 3. Security Updates
```bash
# Check for security vulnerabilities
safety check

# Update vulnerable packages
pip install --upgrade package-name
```

## 📚 Additional Resources

- [PyPI Packaging Guide](https://packaging.python.org/tutorials/packaging-projects/)
- [Maturin Documentation](https://maturin.rs/)
- [Twine Documentation](https://twine.readthedocs.io/)
- [Python Packaging Authority](https://www.pypa.io/)

## 🆘 Support

If you encounter issues during publishing:

1. Check the [GitHub Issues](https://github.com/calimero-network/core/issues)
2. Review the [GitHub Actions logs](https://github.com/calimero-network/core/actions)
3. Contact the team at team@calimero.network

---

**Happy Publishing! 🎉**
