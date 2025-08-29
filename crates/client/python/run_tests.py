#!/usr/bin/env python3
"""
Test runner script for Calimero Python client integration tests.

This script can run tests in different modes:
1. Standalone integration test with Merobox cluster management
2. Pytest-based tests with fixtures
3. Quick verification of bindings
"""

import sys
import argparse
import subprocess
from pathlib import Path


def check_dependencies():
    """Check if all required dependencies are available."""
    print("🔍 Checking dependencies...")
    
    # Check Python version
    if sys.version_info < (3, 8):
        print("❌ Python 3.8+ required")
        return False
    
    # Check if merobox is available
    try:
        import merobox
        print(f"✅ Merobox {merobox.__version__} available")
    except ImportError:
        print("❌ Merobox not found. Please install it first:")
        print("   pip install merobox")
        return False
    
    # Check if PyYAML is available (for workflow tests)
    try:
        import yaml
        print("✅ PyYAML available")
    except ImportError:
        print("⚠️  PyYAML not available (workflow tests will be skipped)")
    
    # Check if pytest is available
    try:
        import pytest
        print(f"✅ Pytest {pytest.__version__} available")
    except ImportError:
        print("⚠️  Pytest not available (pytest tests will be skipped)")
    
    return True


def run_standalone_test():
    """Run the standalone integration test."""
    print("\n🧪 Running standalone integration test...")
    
    # The test file is in the src/tests directory
    test_file = Path(__file__).parent / "src" / "tests" / "test_integration.py"
    if not test_file.exists():
        print(f"❌ Test file not found: {test_file}")
        return False
    
    try:
        # Change to the test directory so imports work correctly
        test_dir = test_file.parent
        result = subprocess.run(
            [sys.executable, str(test_file)], 
            cwd=str(test_dir),
            check=True
        )
        return result.returncode == 0
    except subprocess.CalledProcessError as e:
        print(f"❌ Standalone test failed with exit code: {e.returncode}")
        return False
    except FileNotFoundError:
        print("❌ Python executable not found")
        return False


def run_pytest_tests():
    """Run pytest-based tests."""
    print("\n🧪 Running pytest-based tests...")
    
    # The pytest test file is in the python directory
    test_file = Path(__file__).parent / "test_integration_pytest.py"
    if not test_file.exists():
        print(f"❌ Pytest test file not found: {test_file}")
        return False
    
    try:
        result = subprocess.run([
            sys.executable, "-m", "pytest", 
            str(test_file), "-v", "--tb=short"
        ], check=True)
        return result.returncode == 0
    except subprocess.CalledProcessError as e:
        print(f"❌ Pytest tests failed with exit code: {e.returncode}")
        return False
    except FileNotFoundError:
        print("❌ Python executable not found")
        return False


def run_quick_verification():
    """Run a quick verification of the bindings."""
    print("\n🔍 Running quick binding verification...")
    
    try:
        # Test imports
        from calimero_client_py_bindings import (
            create_connection, create_client, ClientError, AuthMode
        )
        print("✅ All bindings imported successfully")
        
        # Test connection creation
        conn = create_connection("http://localhost:9999", "test")
        print("✅ Connection creation works")
        
        # Test client creation
        client = create_client(conn)
        print("✅ Client creation works")
        
        # List available methods
        methods = [m for m in dir(client) if not m.startswith('_') and callable(getattr(client, m))]
        print(f"✅ Client has {len(methods)} methods available")
        
        return True
        
    except ImportError as e:
        print(f"❌ Import failed: {e}")
        return False
    except Exception as e:
        print(f"❌ Verification failed: {e}")
        return False


def main():
    """Main entry point."""
    parser = argparse.ArgumentParser(description="Calimero Python Client Test Runner")
    parser.add_argument(
        "--mode", 
        choices=["all", "standalone", "pytest", "quick"],
        default="all",
        help="Test mode to run (default: all)"
    )
    parser.add_argument(
        "--verbose", "-v",
        action="store_true",
        help="Verbose output"
    )
    
    args = parser.parse_args()
    
    print("🧪 Calimero Python Client Test Runner")
    print("=" * 50)
    
    # Check dependencies first
    if not check_dependencies():
        print("\n❌ Dependency check failed. Please install missing dependencies.")
        sys.exit(1)
    
    success = True
    
    # Run tests based on mode
    if args.mode in ["all", "quick"]:
        if not run_quick_verification():
            success = False
    
    if args.mode in ["all", "standalone"]:
        if not run_standalone_test():
            success = False
    
    if args.mode in ["all", "pytest"]:
        if not run_pytest_tests():
            success = False
    
    # Print summary
    print("\n" + "=" * 50)
    if success:
        print("🎉 All tests completed successfully!")
        sys.exit(0)
    else:
        print("❌ Some tests failed")
        sys.exit(1)


if __name__ == "__main__":
    main()
