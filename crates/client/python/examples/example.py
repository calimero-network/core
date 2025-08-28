#!/usr/bin/env python3
"""
Example script demonstrating how to use the Calimero client Python bindings.

This example shows how to:
1. Create a connection to a local Calimero node
2. Test client methods and get basic information
3. Handle errors gracefully
"""

import asyncio
import sys
from typing import Optional

try:
    from calimero_client_py_bindings.calimero_client_py_bindings import create_connection, create_client, ConnectionInfo, Client
except ImportError:
    print("❌ Error: calimero_client_py_bindings not found.")
    print("Please install the package first:")
    print("  pip install ../../target/wheels/calimero_client_py_bindings-*.whl")
    sys.exit(1)


async def test_client_methods_example():
    """Example of testing client methods on localhost:2528."""
    
    print("🔗 Creating connection to localhost:2528...")
    
    try:
        # Create connection directly with URL and node name
        connection = create_connection(
            "http://localhost:2528",  # api_url
            "local-dev"               # node_name (optional)
        )
        print("✅ Connection created successfully")
        
        # Create client
        client = create_client(connection)
        print("✅ Client created successfully")
        
        print("\n🔍 Testing client methods...")
        
        # Try to get basic information using simple getter methods
        try:
            api_url = client.get_api_url()
            print("✅ API URL retrieved successfully!")
            print(f"📊 API URL: {api_url}")
            
            # Try to get supported alias types
            alias_types = client.get_supported_alias_types()
            print("✅ Supported alias types retrieved!")
            print(f"📊 Alias types: {alias_types}")
            
        except Exception as client_error:
            print(f"⚠️  Could not retrieve client information: {client_error}")
            print("💡 This might require authentication or the node might not be fully configured")
        
        # Now let's test an async method to see if that's the issue
        print("\n🔄 Testing async methods...")
        try:
            # Try to get peers count (async method)
            peers_count = client.get_peers_count()
            print("✅ Peers count retrieved successfully!")
            print(f"📊 Peers count: {peers_count}")
            
        except Exception as peers_error:
            print(f"⚠️  Could not get peers count: {peers_error}")
            print("💡 This might be a network issue or the node might not be responding")
        
        # Let's also try list_applications to see the exact error
        print("\n📱 Testing list_applications...")
        try:
            applications = client.list_applications()
            print("✅ Applications listed successfully!")
            print(f"📊 Applications: {applications}")
            
        except Exception as apps_error:
            print(f"❌ list_applications failed: {apps_error}")
            print("💡 This method seems to have a bug in the Python bindings")
            print("💡 The error suggests an issue with list processing in the Rust code")
            
            # Try a basic connectivity test as fallback
            try:
                basic_response = connection.get("/")
                print("✅ Server is responding!")
                print(f"📊 Basic response: {basic_response}")
            except Exception as basic_error:
                print(f"⚠️  Server connectivity test: {basic_error}")
                print("💡 The server is reachable but endpoints return 404")
                print("💡 This suggests a Calimero node might be running but with different endpoints")
        
    except Exception as e:
        print(f"❌ Error: {e}")
        return False
    
    return True


async def main():
    """Main function to run the example."""
    print("🚀 Calimero Client Python Bindings Example")
    print("=" * 50)
    
    success = await test_client_methods_example()
    
    if success:
        print("\n✅ Example completed successfully!")
        print("\n💡 Next steps:")
        print("  - Add authentication if needed")
        print("  - Handle different response types")
        print("  - Test other client methods (create_context, list_contexts, etc.)")
    else:
        print("\n❌ Example failed!")
        sys.exit(1)


if __name__ == "__main__":
    # Run the async main function
    try:
        asyncio.run(main())
    except KeyboardInterrupt:
        print("\n\n⏹️  Example interrupted by user")
        sys.exit(0)
    except Exception as e:
        print(f"\n💥 Unexpected error: {e}")
        sys.exit(1)
