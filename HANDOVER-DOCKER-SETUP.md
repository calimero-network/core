# Multi-Node Auth Setup - Handover Document

## Overview
This document provides a comprehensive status review of the multi-node Docker setup with authentication and outlines the remaining tasks for completion.

## Related Pull Requests
- **Core**: [calimero-network/core#1385](https://github.com/calimero-network/core/pull/1385)
- **Admin Dashboard**: [calimero-network/admin-dashboard#122](https://github.com/calimero-network/admin-dashboard/pull/122)
- **Auth Frontend**: [calimero-network/auth-frontend#5](https://github.com/calimero-network/auth-frontend/pull/5/files)
- **Calimero JS Client**: [calimero-network/calimero-client-js#65](https://github.com/calimero-network/calimero-client-js/pull/65/files)

## Current Architecture

### ✅ **Completed: Modular Docker Compose Structure**

The setup has been successfully separated into 4 Docker Compose files for clear separation of concerns:

#### 1. `docker-compose.auth.yml` - Authentication Infrastructure
**Purpose**: Core auth service and Traefik proxy
**Services**:
- `auth` - Authentication service (port 3001)
- `proxy` - Traefik reverse proxy with subdomain routing

**Status**: ✅ **COMPLETE**
- Auth service accessible at `http://localhost/auth/` and `http://localhost/admin/`
- Proxy dashboard at `http://proxy.127.0.0.1.nip.io`
- Health checks implemented
- CORS middleware configured
- ForwardAuth middleware ready for API protection

#### 2. `docker-compose.nodes.yml` - Node Services
**Purpose**: 1-3 Calimero nodes with subdomain routing
**Services**:
- `node1`, `node2`, `node3` - Individual node services

**Status**: ✅ **COMPLETE** (with caveats - see issues below)
- Each node accessible at `nodeX.127.0.0.1.nip.io`
- Auth protection labels configured (dormant when auth not running)
- Admin dashboards publicly accessible
- API/WebSocket endpoints protected via ForwardAuth when auth active

#### 3. `docker-compose.config.yml` - Configuration Services
**Purpose**: Setup, build, and deployment services
**Services**:
- `init_volume` - Volume initialization
- `backend_build` - Application compilation
- `coordinator` - Context and credential management  
- `app_installer` - Application deployment

**Status**: ⚠️ **PARTIALLY COMPLETE** (needs Mira's script integration)

#### 4. `docker-compose.prod.yml` - Legacy Production
**Purpose**: All-in-one setup for simple deployments
**Status**: ✅ **COMPLETE**

## Critical Architecture Review Findings

⚠️ **IMPORTANT**: After thorough review of all components, there are several critical implementation gaps that must be addressed:

### 🚨 **Critical Issue: Network Configuration Mismatch**

**Problem**: `docker-compose.nodes.yml` references external networks that don't exist when running standalone:
```yaml
networks:
  web:
    external: true
    name: ${COMPOSE_PROJECT_NAME:-calimero}_web
  internal:
    external: true  
    name: ${COMPOSE_PROJECT_NAME:-calimero}_internal
```

**Impact**: Standalone nodes (no-auth scenario) will fail to start because they expect networks created by auth service.

**Solution Required**: Networks should be created internally in nodes.yml or made conditional based on auth service availability.

### 🚨 **Critical Issue: Auth Service Reference Without Dependency**

**Problem**: All nodes reference `auth-service` in Traefik labels but have no dependency on auth container:
```yaml
- "traefik.http.routers.node1-auth.service=auth-service"
- "traefik.http.services.auth-service.loadbalancer.server.port=3001"
```

**Impact**: Traefik will show configuration errors when auth service is not available.

**Solution Required**: Make auth-related labels conditional or use Traefik's built-in service discovery.

## Current Issues & Gaps

### 🔧 **Priority 1: Fix Network and Service Dependencies**

**Immediate Action Required**:
1. Fix network configuration for standalone operation
2. Make auth service references conditional
3. Test both scenarios after fixes

### 🔧 **Priority 2: Replace Docker Init with Mira's Scripts**

**Current Problem**: 
- Node initialization in `docker-compose.nodes.yml` uses inline shell commands
- Lines 38-42 in each node service contain basic `merod init` calls

**Required Solution**:
- Replace with Mira's `gen_localnet_configs.sh` script
- Script is located at `crates/merod/gen_localnet_configs.sh`
- Provides proper node initialization with correct port assignments

**Files Affected**:
- `docker-compose.nodes.yml` (lines 38-42, 92-96, 146-150)
- `docker-compose.config.yml` (coordinator service needs script integration)

### 🔧 **Priority 2: Dynamic Client Application Setup**

**Current Problem**:
- Application deployment logic is placeholder-only
- No dynamic setup for contract applications or frontend
- `coordinator` service has placeholder context creation

**Required Solution**:
- Implement dynamic application building and deployment
- Configure frontend applications to connect to appropriate node endpoints
- Handle context creation and member key management automatically

**Files Affected**:
- `docker-compose.config.yml` (coordinator and app_installer services)

### 🔧 **Priority 3: Testing Requirements**

**Current Status**: ⚠️ **UNTESTED**

Both scenarios need comprehensive testing:

## Testing Checklist

### Test Scenario 1: Standalone Nodes (No Auth)
**Command**: `docker-compose -f docker-compose.nodes.yml up`

**❌ CURRENT STATUS: WILL FAIL**
- External network references will cause startup failure
- Auth service references will generate Traefik errors

**Expected Behavior** (after fixes):
- ✅ All 3 nodes start successfully
- ✅ Admin dashboards accessible at:
  - `http://node1.127.0.0.1.nip.io/admin-dashboard`
  - `http://node2.127.0.0.1.nip.io/admin-dashboard`  
  - `http://node3.127.0.0.1.nip.io/admin-dashboard`
- ✅ API endpoints publicly accessible (no auth required)
- ✅ WebSocket connections work without authentication

**Test Actions Needed**:
- [ ] Verify all nodes start without errors
- [ ] Check admin dashboard accessibility
- [ ] Test API calls to `/jsonrpc` and `/admin-api/`
- [ ] Verify WebSocket connections to `/ws`
- [ ] Confirm no auth-related errors in logs

### Test Scenario 2: Auth-Protected Environment
**Commands**: 
```bash
docker-compose -f docker-compose.auth.yml up -d
docker-compose -f docker-compose.nodes.yml up
```

**Expected Behavior**:
- ✅ Auth service starts first and is healthy
- ✅ Nodes connect to auth service successfully  
- ✅ Admin dashboards remain publicly accessible
- ✅ API/WebSocket endpoints require authentication
- ✅ Auth service accessible on all node subdomains:
  - `http://node1.127.0.0.1.nip.io/auth/login`
  - `http://node2.127.0.0.1.nip.io/auth/login`
  - `http://node3.127.0.0.1.nip.io/auth/login`

**Test Actions Needed**:
- [ ] Verify auth service health check passes
- [ ] Test unauthenticated API calls return 401/403
- [ ] Test authenticated API calls work with valid tokens
- [ ] Verify auth UI loads on all node subdomains
- [ ] Test login flow and token validation
- [ ] Check ForwardAuth middleware functionality

## Immediate Next Steps (UPDATED PRIORITIES)

### Step 1: Fix Critical Docker Configuration Issues
**Time Estimate**: 1-2 hours
**Priority**: CRITICAL - Must be done first

#### 1.1 Fix Network Configuration in `docker-compose.nodes.yml`
**Current Issue**: Lines 201-207 reference external networks that don't exist in standalone mode
```yaml
# BROKEN - will fail in standalone mode
networks:
  web:
    external: true
    name: ${COMPOSE_PROJECT_NAME:-calimero}_web
```

**Required Fix**: Make networks conditional or create them internally
```yaml
# OPTION 1: Create networks internally (recommended)
networks:
  web:
    driver: bridge
  internal:
    internal: true

# OPTION 2: Make conditional (more complex)
# Would require environment variable logic
```

#### 1.2 Fix Auth Service References
**Current Issue**: All nodes reference auth service that may not exist (lines 67, 122, 176, 199)

**Required Fix**: Either remove auth references from nodes.yml or make them conditional

#### 1.3 Fix Middleware Duplication
**Current Issue**: CORS and auth-headers middlewares defined multiple times causing conflicts

**Required Fix**: Move all middleware definitions to single location (proxy service in auth.yml)

### Step 2: Integrate Mira's Node Initialization Script
**Time Estimate**: 2-4 hours

1. **Modify `docker-compose.nodes.yml`**:
   - Replace inline init commands with calls to `gen_localnet_configs.sh`
   - Ensure script has access to proper build context
   - Update volume mounts if needed for script access

2. **Update `docker-compose.config.yml`**:
   - Integrate script into coordinator service
   - Ensure proper sequencing with volume initialization

**Implementation Notes**:
- Script uses ports 2428+ for swarm, 2528+ for server (matches current setup)
- Script expects `$HOME/.calimero` as base directory
- May need Docker volume mapping adjustments

### Step 2: Implement Dynamic Application Setup
**Time Estimate**: 4-8 hours

1. **Application Building**:
   - Enhance `backend_build` service for actual app compilation
   - Add build logic for sample applications
   - Ensure proper WASM output handling

2. **Context Management**:
   - Replace placeholder logic in `coordinator` service
   - Implement real context creation using `meroctl`
   - Handle member key generation and storage

3. **Frontend Configuration**:
   - Add dynamic frontend build with correct API endpoints
   - Configure client applications to connect to appropriate nodes
   - Handle environment variable injection for node URLs

### Step 3: Comprehensive Testing
**Time Estimate**: 4-6 hours

1. **No-Auth Testing**:
   - Test all node functionalities without auth
   - Verify network communication between nodes
   - Test application deployment and execution

2. **Auth Testing**:
   - Test complete auth flow
   - Verify API protection works correctly
   - Test multi-node auth scenarios
   - Validate ForwardAuth middleware behavior

## Environment Variables Reference

### Current Variables
- `CONTEXT_RECREATE=true` - Force context recreation
- `COMPOSE_PROFILES=node1|node2|node3` - Control active nodes
- `COMPOSE_PROJECT_NAME` - Override project name (default: calimero)
- `NODE1_URL`, `NODE2_URL`, `NODE3_URL` - Node endpoints for configuration
- `APP_PATH` - Application WASM file path
- `NODE_NAME` - Target node for application installation

### Variables That May Need Addition
- `ENABLE_AUTH=true/false` - Toggle auth requirements
- `DEFAULT_CONTEXT_ID` - Reuse existing context
- `BUILD_APPS=true/false` - Control application building
- `FRONTEND_BUILD_PATH` - Frontend build output location

## Potential Issues & Solutions

### Issue 1: Network Timing
**Problem**: Nodes may start before auth service is fully ready
**Solution**: Add proper health check dependencies and startup delays

### Issue 2: Volume Permissions
**Problem**: Docker volume permissions may cause init script failures
**Solution**: Ensure `init_volume` service sets correct permissions for script execution

### Issue 3: Port Conflicts
**Problem**: Multiple compose files may conflict on port usage
**Solution**: Document proper startup sequence and add port conflict detection

## Success Criteria

### Functional Requirements
- [ ] Standalone nodes run without auth dependency
- [ ] Auth-protected setup enforces access control
- [ ] All admin dashboards are accessible
- [ ] API/WebSocket protection works correctly
- [ ] Application deployment succeeds
- [ ] Multi-node communication works

### Operational Requirements  
- [ ] Setup process is documented and repeatable
- [ ] Error messages are clear and actionable
- [ ] Cleanup process removes all resources
- [ ] Performance is acceptable for development use

## Documentation Updates Needed

1. **Update `README-docker-setup.md`**:
   - Add testing instructions for both scenarios
   - Document troubleshooting steps
   - Include performance expectations

2. **Create Quick Start Guide**:
   - Simple commands for common use cases
   - Clear success/failure indicators
   - Common error solutions

3. **Add Development Workflow**:
   - Application development cycle
   - Context management best practices
   - Multi-node development tips

---

## Summary for Handover

### ✅ What's Complete and Working
1. **Modular Docker Architecture**: 4 separate compose files with clear separation of concerns
2. **Auth Service**: Fully functional with health checks and Traefik configuration  
3. **Production Setup**: Legacy all-in-one compose file ready for simple deployments
4. **Documentation**: Comprehensive setup guide and this handover document

### ❌ Critical Issues That Must Be Fixed
1. **Network Configuration**: Standalone nodes will fail due to external network references
2. **Auth Service Dependencies**: Node services reference non-existent auth service
3. **Middleware Conflicts**: Duplicate Traefik middleware definitions

### ⚠️ Implementation Gaps
1. **Node Initialization**: Using basic Docker commands instead of Mira's proper scripts
2. **Application Deployment**: Placeholder logic needs real implementation
3. **Dynamic Configuration**: No dynamic setup for client applications

### 🧪 Testing Status
- **No scenarios tested yet** - Both standalone and auth-protected setups need validation
- **Estimated effort**: 8-12 hours to fix critical issues and complete implementation
- **Risk level**: Medium - architecture is sound but implementation has blocking issues

### 📋 Next Developer Action Plan
1. **Day 1**: Fix network and service reference issues (1-2 hours)
2. **Day 1-2**: Test both scenarios to validate fixes (2-3 hours)  
3. **Day 2-3**: Integrate Mira's scripts (2-4 hours)
4. **Day 3-4**: Implement dynamic application setup (4-8 hours)
5. **Day 4**: Final testing and documentation (2-3 hours)

**CRITICAL**: Do not attempt end-to-end testing until Step 1 network issues are resolved.

---

## Contact & Support

- **Primary Contact**: [Current Developer]
- **Repository**: calimero-network/core
- **Documentation**: README-docker-setup.md
- **Issues**: GitHub Issues for respective repositories

Last Updated: January 2025
Status: Architecture complete, critical fixes needed before testing
