#!/usr/bin/env python3
"""
Setup script for Calimero Client Python Bindings

This file provides an alternative to pyproject.toml for traditional Python packaging.
The primary build system is maturin, but this allows for additional setup options.
"""

from setuptools import setup, find_packages
import os

# Read the README file
def read_readme():
    with open("README.md", "r", encoding="utf-8") as fh:
        return fh.read()

# Read requirements
def read_requirements(filename):
    with open(filename, "r", encoding="utf-8") as fh:
        return [line.strip() for line in fh if line.strip() and not line.startswith("#")]

setup(
    name="calimero-client-py-bindings",
    version="0.1.0",
    author="Calimero Network",
    author_email="team@calimero.network",
    description="Python client library for Calimero Network - a comprehensive blockchain API client",
    long_description=read_readme(),
    long_description_content_type="text/markdown",
    url="https://calimero.network",
    project_urls={
        "Homepage": "https://calimero.network",
        "Repository": "https://github.com/calimero-network/core",
        "Documentation": "https://docs.calimero.network",
        "Bug Tracker": "https://github.com/calimero-network/core/issues",
        "Changelog": "https://github.com/calimero-network/core/blob/main/CHANGELOG.md",
        "Discussions": "https://github.com/calimero-network/core/discussions",
    },
    packages=find_packages(where="src"),
package_dir={"": "src"},
    classifiers=[
        "Development Status :: 4 - Beta",
        "Intended Audience :: Developers",
        "License :: OSI Approved :: MIT License",
        "Operating System :: OS Independent",
        "Programming Language :: Python :: 3",
        "Programming Language :: Python :: 3.8",
        "Programming Language :: Python :: 3.9",
        "Programming Language :: Python :: 3.10",
        "Programming Language :: Python :: 3.11",
        "Programming Language :: Python :: 3.12",
        "Programming Language :: Python :: 3.13",
        "Programming Language :: Python :: Implementation :: CPython",
        "Programming Language :: Python :: Implementation :: PyPy",
        "Topic :: Software Development :: Libraries :: Python Modules",
        "Topic :: Internet :: WWW/HTTP :: HTTP Clients",
        "Topic :: Database :: Database Engines/Servers",
        "Topic :: System :: Distributed Computing",
        "Topic :: System :: Networking",
        "Topic :: Utilities",
        "Typing :: Typed",
    ],
    python_requires=">=3.8",
    install_requires=[
        "typing-extensions>=4.0.0; python_version<'3.8'",
        "asyncio-mqtt>=0.16.0; python_version<'3.11'",
    ],
    extras_require={
        "dev": [
            "pytest>=7.0.0",
            "pytest-asyncio>=0.21.0",
            "pytest-cov>=4.0.0",
            "black>=23.0.0",
            "isort>=5.12.0",
            "flake8>=6.0.0",
            "mypy>=1.0.0",
            "pre-commit>=3.0.0",
        ],
        "docs": [
            "sphinx>=6.0.0",
            "sphinx-rtd-theme>=1.2.0",
            "myst-parser>=1.0.0",
        ],
        "test": [
            "pytest>=7.0.0",
            "pytest-asyncio>=0.21.0",
            "pytest-cov>=4.0.0",
            "pytest-mock>=3.10.0",
            "httpx>=0.24.0",
            "responses>=0.23.0",
        ],
    },
    entry_points={
        "console_scripts": [
            "calimero-client-py=calimero_client_py_bindings.cli:main",
        ],
    },
    keywords=[
        "calimero",
        "blockchain",
        "api",
        "client",
        "web3",
        "distributed-ledger",
        "cryptocurrency",
        "defi",
    ],
    license="MIT",
    zip_safe=False,
    include_package_data=True,
    package_data={
        "calimero_client_py_bindings": ["py.typed"],
    },
)
