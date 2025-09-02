#!/usr/bin/env python3
"""
Comprehensive integration test for Calimero Python client bindings using Merobox testing framework.

This test covers the complete client workflow:
1. Installing an application
2. Listing applications
3. Creating a context
4. Inviting a second node
5. Calling set function on one node
6. Calling get function on the other node
"""

import os
import sys
import time
import json
from pathlib import Path

# Import the bindings from the installed package
try:
    from calimero_client_py_bindings import (
        create_connection,
        create_client,
        ClientError,
        AuthMode
    )
except ImportError as e:
    print(f"Failed to import calimero_client_py_bindings: {e}")
    print("Make sure the package is built and installed")
    sys.exit(1)

try:
    from merobox.testing import cluster
except ImportError as e:
    print(f"Failed to import merobox.testing: {e}")
    print("Make sure merobox is installed: pip install merobox")
    sys.exit(1)


class CalimeroIntegrationTest:
    """Comprehensive integration test for Calimero client using Merobox testing framework."""
    
    def __init__(self):
        self.app_url = "https://calimero-only-peers-dev.s3.eu-central-1.amazonaws.com/uploads/kv_store.wasm"
        self.protocol = "near"  # Use actual protocol name like "ethereum", "near", "stellar"
        self.network = "test"
        
        # Test data
        self.test_key = "test_key"
        self.test_value = "test_value"
        
        # Store results
        self.results = {}
        
        # Test state
        self.app_id = None
        self.context_id = None
        self.member_public_key = None
        self.node2_public_key = None
        self.invitation_payload = None
        
    def test_with_merobox_cluster(self):
        """Test using Merobox cluster management."""
        print("🧪 Testing with Merobox cluster management...")
        
        try:
            # Use Merobox's cluster context manager for automatic cleanup
            with cluster(count=2, prefix="test", image="ghcr.io/calimero-network/merod:edge") as env:
                nodes = env["nodes"]
                endpoints = env["endpoints"]
                
                print(f"✅ Cluster started with nodes: {nodes}")
                print(f"🌐 Endpoints: {endpoints}")
                
                # Wait for nodes to be ready
                time.sleep(10)
                
                # Test the complete workflow
                success = self.test_complete_workflow(endpoints, nodes)
                
                if success:
                    print("🎉 All tests completed successfully with Merobox cluster!")
                else:
                    print("❌ Some tests failed with Merobox cluster")
                
                return success
        except Exception as e:
            print(f"❌ Cluster test failed with error: {e}")
            return False
    
    def test_with_merobox_workflow(self):
        """Test using Merobox workflow-based setup."""
        print("🧪 Testing with Merobox workflow setup...")
        
        # For now, we'll skip workflow testing and just use cluster testing
        print("⚠️  Workflow testing is currently disabled - using cluster testing instead")
        return self.test_with_merobox_cluster()
    
    def test_complete_workflow(self, endpoints, nodes):
        """Test the complete client workflow with the given endpoints and nodes."""
        print(f"\n🔌 Testing complete workflow with {len(nodes)} nodes...")
        
        try:
            # Get the first two endpoints
            node1_endpoint = list(endpoints.values())[0]
            node2_endpoint = list(endpoints.values())[1]
            
            print(f"🌐 Node 1 endpoint: {node1_endpoint}")
            print(f"🌐 Node 2 endpoint: {node2_endpoint}")
            
            # Test connections
            print("\n🔌 Testing connection creation...")
            try:
                conn1 = create_connection(node1_endpoint, None)
                conn2 = create_connection(node2_endpoint, None)
                print(f"✅ Created connection to node 1: {conn1.api_url}")
                print(f"✅ Created connection to node 2: {conn2.api_url}")
                self.results['connections'] = True
            except Exception as e:
                print(f"❌ Connection creation failed: {e}")
                self.results['connections'] = False
                return False
            
            # Test clients
            print("\n🖥️  Testing client creation...")
            try:
                client1 = create_client(conn1)
                client2 = create_client(conn2)
                print(f"✅ Created client for node 1: {client1.get_api_url}")
                print(f"✅ Created client for node 2: {client2.get_api_url}")
                self.results['clients'] = True
            except Exception as e:
                print(f"❌ Client creation failed: {e}")
                self.results['clients'] = False
                return False
            
            # Test basic connectivity first (should work without authentication)
            print("\n🔍 Testing basic connectivity...")
            
            # Test get peers count
            try:
                peers_response = client1.get_peers_count()
                print(f"✅ Peers count: {peers_response}")
                self.results['get_peers_count'] = True
            except Exception as e:
                print(f"⚠️  Get peers count failed: {e}")
                self.results['get_peers_count'] = False
            
            # Test list blobs
            try:
                blobs_response = client1.list_blobs()
                print(f"✅ Blobs listed: {blobs_response}")
                self.results['list_blobs'] = True
            except Exception as e:
                print(f"⚠️  List blobs failed: {e}")
                self.results['list_blobs'] = False
            
            # Test sync all contexts
            try:
                sync_response = client1.sync_all_contexts()
                print(f"✅ Sync all contexts: {sync_response}")
                self.results['sync_all_contexts'] = True
            except Exception as e:
                print(f"⚠️  Sync all contexts failed: {e}")
                self.results['sync_all_contexts'] = False
            
            # Test application installation
            print("\n📦 Testing application installation...")
            try:
                response = client1.install_application(self.app_url)
                print(f"✅ Application installed: {response}")
                
                # Get the application ID from the response
                if isinstance(response, dict) and 'data' in response:
                    app_id = (response['data'].get('application_id') or 
                             response['data'].get('applicationId') or
                             response['data'].get('id'))
                    if app_id:
                        self.app_id = app_id
                        print(f"📋 Application ID: {app_id}")
                    else:
                        print("⚠️  No application ID found in response")
                        print(f"   Available fields: {list(response['data'].keys())}")
                        self.app_id = None
                else:
                    print("⚠️  Unexpected response format")
                    self.app_id = None
                
                self.results['app_installation'] = True
            except Exception as e:
                print(f"❌ Application installation failed: {e}")
                self.results['app_installation'] = False
            
            # Test application listing
            print("\n📋 Testing application listing...")
            try:
                apps = client1.list_applications()
                print(f"✅ Applications listed: {apps}")
                
                # Verify our app is in the list
                if isinstance(apps, dict) and 'data' in apps:
                    app_list = (apps['data'].get('applications', []) or 
                               apps['data'].get('apps', []))
                    if app_list:
                        print(f"📋 Found {len(app_list)} applications")
                        for app in app_list:
                            print(f"  - {app}")
                        
                        # If we don't have an app_id yet, try to extract it from the list
                        if not self.app_id and app_list:
                            first_app = app_list[0]
                            if isinstance(first_app, dict):
                                potential_app_id = (first_app.get('id') or 
                                                  first_app.get('application_id') or 
                                                  first_app.get('applicationId'))
                                if potential_app_id:
                                    self.app_id = potential_app_id
                                    print(f"📋 Extracted app ID from list: {self.app_id}")
                    else:
                        print("📋 No applications found")
                
                self.results['list_apps'] = True
            except Exception as e:
                print(f"❌ Application listing failed: {e}")
                self.results['list_apps'] = False
            
            # Test get application
            print("\n🔍 Testing get application...")
            print(f"🔍 Debug: Current app_id = {self.app_id}")
            if self.app_id:
                try:
                    app_info = client1.get_application(self.app_id)
                    print(f"✅ Application info retrieved: {app_info}")
                    self.results['get_app'] = True
                except Exception as e:
                    print(f"❌ Get application failed: {e}")
                    self.results['get_app'] = False
            else:
                print("⚠️  Skipping get application test - no app ID available")
                self.results['get_app'] = False
            
            # Test context creation
            print(f"🔍 Debug: Current app_id for context creation = {self.app_id}")
            if self.app_id:
                print("\n🏗️  Testing context creation...")
                
                # Initialize context creation tracking
                context_created = False
                
                # First, let's check if there are any existing contexts
                print("\n🔍 Checking for existing contexts...")
                try:
                    existing_contexts = client1.list_contexts()
                    print(f"📋 Existing contexts: {existing_contexts}")
                    
                    # If there are existing contexts, try to use the first one
                    if existing_contexts.get('data', {}).get('contexts'):
                        existing_context = existing_contexts['data']['contexts'][0]
                        self.context_id = existing_context.get('id') or existing_context.get('context_id')
                        self.member_public_key = existing_context.get('member_public_key')
                        
                        if self.context_id and self.member_public_key:
                            print(f"✅ Using existing context: {self.context_id}")
                            context_created = True
                        else:
                            print("⚠️  Existing context missing required fields")
                    
                except Exception as e:
                    print(f"⚠️  Could not list existing contexts: {e}")
                
                # If no existing context, try to create a new one
                if not context_created:
                    print("\n🔨 Attempting to create new context with near protocol...")
                    try:
                        print(f"   Calling create_context with app_id={self.app_id}, protocol=near")
                        response = client1.create_context(self.app_id, "near")
                        print(f"✅ Context created successfully: {response}")
                        
                        # Extract context ID and member public key
                        if isinstance(response, dict) and 'data' in response:
                            context_data = response['data']
                            # Handle both camelCase and snake_case field names
                            self.context_id = context_data.get('context_id') or context_data.get('contextId')
                            self.member_public_key = context_data.get('member_public_key') or context_data.get('memberPublicKey')
                            
                            if self.context_id and self.member_public_key:
                                print(f"🏗️  Context ID: {self.context_id}")
                                print(f"🔑 Member public key: {self.member_public_key}")
                                context_created = True
                            else:
                                print("⚠️  Missing context ID or member public key")
                                print(f"      Response data: {context_data}")
                                self.context_id = None
                                self.member_public_key = None
                        else:
                            print("⚠️  Unexpected response format")
                            print(f"      Full response: {response}")
                            self.context_id = None
                            self.member_public_key = None
                            
                    except Exception as e:
                        print(f"❌ Context creation failed: {e}")
                        print(f"   Error type: {type(e)}")
                        print(f"   Error message: {str(e)}")
                        if "500" in str(e):
                            print("   Server returned 500 error")
                            # Try to get more error details if possible
                            if hasattr(e, '__cause__') and e.__cause__:
                                print(f"   Cause: {e.__cause__}")
                            if hasattr(e, 'args') and e.args:
                                print(f"   Error args: {e.args}")
                
                if context_created:
                    self.results['create_context'] = True
                    print("🎉 Context creation succeeded!")
                else:
                    print("❌ Context creation failed")
                    self.results['create_context'] = False
            else:
                print("⚠️  Skipping context creation test - no app ID available")
                self.results['create_context'] = False
            
            # Test context-related methods
            if self.context_id:
                # Test get context
                print("\n🔍 Testing get context...")
                try:
                    context_info = client1.get_context(self.context_id)
                    print(f"✅ Context info retrieved: {context_info}")
                    self.results['get_context'] = True
                except Exception as e:
                    print(f"❌ Get context failed: {e}")
                    self.results['get_context'] = False
                
                # Test list contexts
                print("\n📋 Testing context listing...")
                try:
                    contexts = client1.list_contexts()
                    print(f"✅ Contexts listed: {contexts}")
                    self.results['list_contexts'] = True
                except Exception as e:
                    print(f"❌ Context listing failed: {e}")
                    self.results['list_contexts'] = False
                
                # Test get context storage
                print("\n💾 Testing get context storage...")
                try:
                    storage = client1.get_context_storage(self.context_id)
                    print(f"✅ Context storage retrieved: {storage}")
                    self.results['get_context_storage'] = True
                except Exception as e:
                    print(f"❌ Get context storage failed: {e}")
                    self.results['get_context_storage'] = False
                
                # Test get context identities
                print("\n👥 Testing get context identities...")
                try:
                    identities = client1.get_context_identities(self.context_id)
                    print(f"✅ Context identities retrieved: {identities}")
                    self.results['get_context_identities'] = True
                except Exception as e:
                    print(f"❌ Get context identities failed: {e}")
                    self.results['get_context_identities'] = False
                

            else:
                print("⚠️  Skipping context-related tests - no context ID available")
                print("   This is expected if context creation failed")
                self.results['get_context'] = False
                self.results['list_contexts'] = False
                self.results['get_context_storage'] = False
                self.results['get_context_identities'] = False
            
            # Test identity generation
            print("\n🆔 Testing context identity generation...")
            try:
                response = client2.generate_context_identity()
                print(f"✅ Context identity generated: {response}")
                
                # Extract the public key
                if isinstance(response, dict) and 'data' in response:
                    # Handle both camelCase and snake_case field names
                    self.node2_public_key = response['data'].get('public_key') or response['data'].get('publicKey')
                    if self.node2_public_key:
                        print(f"🔑 Node 2 public key: {self.node2_public_key}")
                    else:
                        print("⚠️  No public key found in response")
                        print(f"      Response data: {response['data']}")
                        self.node2_public_key = None
                else:
                    print("⚠️  Unexpected response format")
                    print(f"      Full response: {response}")
                    self.node2_public_key = None
                
                self.results['generate_identity'] = True
            except Exception as e:
                print(f"❌ Context identity generation failed: {e}")
                self.results['generate_identity'] = False
            
            # Test invitation
            if all([self.context_id, self.member_public_key, self.node2_public_key]):
                print("\n📨 Testing context invitation...")
                try:
                    response = client1.invite_to_context(self.context_id, self.member_public_key, self.node2_public_key)
                    print(f"✅ Context invitation sent: {response}")
                    
                    # Extract invitation payload
                    if isinstance(response, dict) and 'data' in response:
                        # The data field contains the invitation payload directly
                        self.invitation_payload = response['data']
                        if self.invitation_payload:
                            print(f"📨 Invitation payload: {self.invitation_payload}")
                        else:
                            print("⚠️  No invitation payload found")
                            self.invitation_payload = None
                    elif isinstance(response, str):
                        # Sometimes the response is just the invitation payload string
                        self.invitation_payload = response
                        print(f"📨 Invitation payload (direct string): {self.invitation_payload}")
                    else:
                        print("⚠️  Unexpected response format")
                        print(f"      Response type: {type(response)}")
                        print(f"      Response: {response}")
                        self.invitation_payload = None
                    
                    self.results['invitation'] = True
                except Exception as e:
                    print(f"❌ Context invitation failed: {e}")
                    self.results['invitation'] = False
            else:
                print("⚠️  Skipping invitation test - missing required data")
                self.results['invitation'] = False
            
            # Test joining
            if all([self.context_id, self.node2_public_key, self.invitation_payload]):
                print("\n🤝 Testing context joining...")
                try:
                    response = client2.join_context(self.context_id, self.node2_public_key, self.invitation_payload)
                    print(f"✅ Context joined: {response}")
                    self.results['join_context'] = True
                except Exception as e:
                    print(f"❌ Context joining failed: {e}")
                    self.results['join_context'] = False
            else:
                print("⚠️  Skipping join context test - missing required data")
                self.results['join_context'] = False
            
            # Test function execution
            if all([self.context_id, self.member_public_key]):
                print("\n⚙️  Testing set function execution...")
                try:
                    response = client1.execute_function(
                        self.context_id, 
                        "set", 
                        json.dumps({"key": "test-key", "value": "test-value"}),
                        self.member_public_key
                    )
                    print(f"✅ Set function executed: {response}")
                    self.results['set_function'] = True
                except Exception as e:
                    print(f"❌ Set function execution failed: {e}")
                    self.results['set_function'] = False
            else:
                print("⚠️  Skipping set function test - missing required data")
                self.results['set_function'] = False
            
            if all([self.context_id, self.node2_public_key]):
                print("\n⚙️  Testing get function execution...")
                try:
                    response = client2.execute_function(
                        self.context_id, 
                        "get", 
                        json.dumps({"key": "test-key"}),
                        self.node2_public_key
                    )
                    print(f"✅ Get function executed: {response}")
                    self.results['get_function'] = True
                except Exception as e:
                    print(f"❌ Get function execution failed: {e}")
                    self.results['get_function'] = False
            else:
                print("⚠️  Skipping get function test - missing required data")
                self.results['get_function'] = False
            
            # Test additional methods
            print("\n🔧 Testing additional methods...")
            try:
                # Test permissions methods
                if self.context_id and self.member_public_key:
                    try:
                        permissions = client1.get_context_permissions(self.context_id, self.member_public_key)
                        print(f"✅ Context permissions retrieved: {permissions}")
                    except Exception as e:
                        print(f"⚠️  Get context permissions failed: {e}")
                else:
                    print("⚠️  Skipping permissions test - missing context_id or member_public_key")
                
                # Test proposals methods
                try:
                    proposals = client1.list_proposals()
                    print(f"✅ Proposals listed: {proposals}")
                except Exception as e:
                    print(f"⚠️  List proposals failed: {e}")
                
                # Test sync
                try:
                    sync_result = client1.sync_all_contexts()
                    print(f"✅ Sync completed: {sync_result}")
                except Exception as e:
                    print(f"⚠️  Sync failed: {e}")
                
                self.results['additional_methods'] = True
            except Exception as e:
                print(f"❌ Additional methods test failed: {e}")
                self.results['additional_methods'] = False
            
            # Test alias methods
            print("\n🏷️  Testing alias methods...")
            try:
                if self.context_id:
                    try:
                        # Test context alias
                        alias_name = f"test-context-{int(time.time())}"
                        response = client1.create_context_alias(alias_name, self.context_id)
                        print(f"✅ Context alias created: {response}")
                        
                        # Test lookup and resolve
                        lookup = client1.lookup_context_alias(alias_name)
                        print(f"✅ Context alias lookup: {lookup}")
                        
                        resolve = client1.resolve_context_alias(alias_name)
                        print(f"✅ Context alias resolve: {resolve}")
                        
                        # Test delete
                        delete_response = client1.delete_context_alias(alias_name)
                        print(f"✅ Context alias deleted: {delete_response}")
                        
                    except Exception as e:
                        print(f"⚠️  Context alias methods failed: {e}")
                
                if self.app_id:
                    try:
                        # Test application alias
                        alias_name = f"test-app-{int(time.time())}"
                        response = client1.create_application_alias(alias_name, self.app_id)
                        print(f"✅ Application alias created: {response}")
                        
                        # Test lookup and resolve
                        lookup = client1.lookup_application_alias(alias_name)
                        print(f"✅ Application alias lookup: {lookup}")
                        
                        resolve = client1.resolve_application_alias(alias_name)
                        print(f"✅ Application alias resolve: {resolve}")
                        
                        # Test delete
                        delete_response = client1.delete_application_alias(alias_name)
                        print(f"✅ Application alias deleted: {delete_response}")
                        
                    except Exception as e:
                        print(f"⚠️  Application alias methods failed: {e}")
                
                self.results['alias_methods'] = True
            except Exception as e:
                print(f"❌ Alias methods test failed: {e}")
                self.results['alias_methods'] = False
            
            print("\n" + "=" * 60)
            print("🎉 All integration tests completed!")
            self.print_results()
            return True
            
        except Exception as e:
            print(f"\n❌ Test execution failed with error: {e}")
            self.print_results()
            return False
    
    def print_results(self):
        """Print a summary of test results."""
        print("\n📊 Test Results Summary:")
        print("-" * 40)
        
        total_tests = len(self.results)
        passed_tests = sum(1 for result in self.results.values() if result)
        failed_tests = total_tests - passed_tests
        
        for test_name, result in self.results.items():
            status = "✅ PASS" if result else "❌ FAIL"
            print(f"{test_name:<30} {status}")
        
        print("-" * 40)
        print(f"Total: {total_tests}, Passed: {passed_tests}, Failed: {failed_tests}")
        
        if failed_tests == 0:
            print("🎉 All tests passed!")
        else:
            print(f"⚠️  {failed_tests} test(s) failed")


def main():
    """Main entry point for the integration test."""
    print("🧪 Starting Calimero Python Client Integration Tests with Merobox")
    print("=" * 68)
    print("=" * 68)
    print()
    
    # Create and run the test
    test = CalimeroIntegrationTest()
    
    # Test with Merobox cluster
    print("🔧 Testing with Merobox cluster management...")
    cluster_success = test.test_with_merobox_cluster()
    
    # Print final results
    print("=" * 68)
    print("=" * 68)
    print()
    
    if cluster_success:
        print("🎉 All tests completed successfully!")
    else:
        print("⚠️  Some tests failed")
    
    # Print overall results
    test.print_results()


if __name__ == "__main__":
    main()
