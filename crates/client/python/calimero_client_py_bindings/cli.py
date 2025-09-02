#!/usr/bin/env python3
"""
Command-line interface for Calimero Client Python Bindings

This module provides a CLI for interacting with Calimero Network APIs
from the command line.
"""

import argparse
import asyncio
import json
import sys
from typing import Optional

from . import create_connection, create_client, ClientError


def create_parser() -> argparse.ArgumentParser:
    """Create the command line argument parser."""
    parser = argparse.ArgumentParser(
        description="Calimero Client - Python bindings CLI",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
Examples:
  # Check API health
  calimero-client-py health --api-url https://api.calimero.network

  # List supported alias types
  calimero-client-py aliases --api-url https://api.calimero.network --node-name my-node

  # Make a custom request
  calimero-client-py request --method GET --endpoint /api/v1/status --api-url https://api.calimero.network
        """
    )
    
    parser.add_argument(
        "--version",
        action="version",
        version="calimero-client-py-bindings 0.1.0"
    )
    
    # Global options
    parser.add_argument(
        "--api-url",
        required=True,
        help="Calimero API base URL"
    )
    
    parser.add_argument(
        "--node-name",
        help="Node name for the connection"
    )
    
    parser.add_argument(
        "--auth-token",
        help="JWT authentication token"
    )
    
    parser.add_argument(
        "--verbose", "-v",
        action="store_true",
        help="Enable verbose output"
    )
    
    # Subcommands
    subparsers = parser.add_subparsers(dest="command", help="Available commands")
    
    # Health check command
    health_parser = subparsers.add_parser("health", help="Check API health")
    
    # Aliases command
    aliases_parser = subparsers.add_parser("aliases", help="List supported alias types")
    
    # Custom request command
    request_parser = subparsers.add_parser("request", help="Make a custom HTTP request")
    request_parser.add_argument("--method", choices=["GET", "POST", "PUT", "DELETE"], default="GET", help="HTTP method")
    request_parser.add_argument("--endpoint", required=True, help="API endpoint (e.g., /api/v1/status)")
    request_parser.add_argument("--data", help="Request data (JSON string)")
    
    return parser


async def health_check(api_url: str, node_name: Optional[str], auth_token: Optional[str], verbose: bool):
    """Check the health of the Calimero API."""
    try:
        connection = create_connection(api_url, node_name)
        if verbose:
            print(f"Checking health at: {api_url}")
        
        response = await connection.get("/health")
        
        if verbose:
            print(f"Response status: {response.status_code}")
            print(f"Response headers: {response.headers}")
        
        print("✅ API is healthy")
        if response.text:
            print(f"Response: {response.text}")
            
    except ClientError as e:
        print(f"❌ Health check failed: {e}")
        sys.exit(1)
    except Exception as e:
        print(f"❌ Unexpected error: {e}")
        sys.exit(1)


async def list_aliases(api_url: str, node_name: Optional[str], auth_token: Optional[str], verbose: bool):
    """List supported alias types."""
    try:
        connection = create_connection(api_url, node_name)
        client = create_client(connection)
        
        if verbose:
            print(f"Fetching alias types from: {api_url}")
        
        alias_types = await client.get_supported_alias_types()
        
        print("📋 Supported Alias Types:")
        for alias_type in alias_types:
            print(f"  - {alias_type}")
            
    except ClientError as e:
        print(f"❌ Failed to fetch alias types: {e}")
        sys.exit(1)
    except Exception as e:
        print(f"❌ Unexpected error: {e}")
        sys.exit(1)


async def make_request(api_url: str, node_name: Optional[str], auth_token: Optional[str], 
                      method: str, endpoint: str, data: Optional[str], verbose: bool):
    """Make a custom HTTP request."""
    try:
        connection = create_connection(api_url, node_name)
        
        if verbose:
            print(f"Making {method} request to: {api_url}{endpoint}")
            if data:
                print(f"Request data: {data}")
        
        if method == "GET":
            response = await connection.get(endpoint)
        elif method == "POST":
            request_data = json.loads(data) if data else {}
            response = await connection.post(endpoint, request_data)
        elif method == "PUT":
            request_data = json.loads(data) if data else {}
            response = await connection.put(endpoint, request_data)
        elif method == "DELETE":
            response = await connection.delete(endpoint)
        else:
            print(f"❌ Unsupported method: {method}")
            sys.exit(1)
        
        print(f"✅ {method} {endpoint}")
        print(f"Status: {response.status_code}")
        
        if response.text:
            try:
                # Try to pretty-print JSON
                json_data = json.loads(response.text)
                print("Response:")
                print(json.dumps(json_data, indent=2))
            except json.JSONDecodeError:
                print(f"Response: {response.text}")
                
    except ClientError as e:
        print(f"❌ Request failed: {e}")
        sys.exit(1)
    except Exception as e:
        print(f"❌ Unexpected error: {e}")
        sys.exit(1)


async def main():
    """Main CLI entry point."""
    parser = create_parser()
    args = parser.parse_args()
    
    if not args.command:
        parser.print_help()
        sys.exit(1)
    
    if args.verbose:
        print(f"🔧 Calimero Client CLI")
        print(f"API URL: {args.api_url}")
        print(f"Node Name: {args.node_name or 'None'}")
        print(f"Auth Token: {'Yes' if args.auth_token else 'No'}")
        print()
    
    try:
        if args.command == "health":
            await health_check(args.api_url, args.node_name, args.auth_token, args.verbose)
        elif args.command == "aliases":
            await list_aliases(args.api_url, args.node_name, args.auth_token, args.verbose)
        elif args.command == "request":
            await make_request(args.api_url, args.node_name, args.auth_token, 
                             args.method, args.endpoint, args.data, args.verbose)
        else:
            print(f"❌ Unknown command: {args.command}")
            sys.exit(1)
            
    except KeyboardInterrupt:
        print("\n⚠️  Operation cancelled by user")
        sys.exit(130)
    except Exception as e:
        print(f"❌ Fatal error: {e}")
        if args.verbose:
            import traceback
            traceback.print_exc()
        sys.exit(1)


def run():
    """Entry point for setuptools."""
    asyncio.run(main())


if __name__ == "__main__":
    run()
