# 🧪 Calimero Workflows

This directory contains automated test workflows for the Calimero Core system. Each workflow tests specific aspects of the system's functionality, performance, and reliability.

## 📋 **Workflow Categories**

### **🔧 Core Functionality Tests**
Basic system functionality and CRDT operations.

### **⚡ Performance Tests**
Measure propagation times and optimization effectiveness.

### **🔄 Convergence Tests**
Verify CRDT convergence under various conditions.

---

## 🚀 **Workflow Details**

### **Core Functionality Tests**

#### **`bootstrap.yml`** - Full System Bootstrap
- **Purpose**: Complete system setup and comprehensive functionality test
- **Nodes**: 8 nodes
- **Duration**: ~3-5 minutes
- **Tests**: 
  - Application installation
  - Context creation and management
  - Identity creation and invitation system
  - Context joining and synchronization
  - Basic CRUD operations
  - State propagation between all nodes
  - Large-scale CRDT operations
  - Multiple concurrent operations
  - Memory and CPU usage validation
- **Use Case**: Production readiness validation, comprehensive integration testing

#### **`bootstrap-short.yml`** - Quick Bootstrap
- **Purpose**: Fast system validation for development
- **Nodes**: 3 nodes
- **Duration**: ~1-2 minutes
- **Tests**: 
  - Basic system setup
  - Core CRDT operations
  - State synchronization
  - Quick performance validation
- **Use Case**: Development testing, quick validation

#### **`kv-store-simple.yml`** - Simple CRDT Test
- **Purpose**: Basic CRDT functionality validation
- **Nodes**: 3 nodes
- **Duration**: ~2-3 minutes
- **Tests**: 
  - CRDT propagation
  - State consistency
  - Basic operations (set, get, entries)
  - Single message propagation time
  - Basic performance metrics
- **Use Case**: Basic functionality testing, debugging, performance validation

---

### **Performance Tests**

#### **`phase2-performance-test.yml`** - Phase 2 Optimizations
- **Purpose**: Test Phase 2 performance optimizations
- **Nodes**: 5 nodes
- **Duration**: ~3-4 minutes
- **Tests**: 
  - Lightweight delta processing
  - Batch processing infrastructure
  - Parallel delta processing
  - Sub-100ms propagation validation
  - Optimistic concurrency control
- **Use Case**: Phase 2 optimization validation

#### **`phase3-performance-test.yml`** - Phase 3 Optimizations
- **Purpose**: Test Phase 3 advanced optimizations
- **Nodes**: 8 nodes
- **Duration**: ~4-5 minutes
- **Tests**: 
  - Direct P2P communication
  - Binary protocol optimization
  - Load balancing under concurrent load
  - Large payload handling (200+ characters)
  - Enterprise-scale performance
  - Connection pooling simulation
- **Use Case**: Advanced optimization validation, enterprise scale testing

---

### **Convergence Tests**

#### **`convergence-test.yml`** - CRDT Convergence Test
- **Purpose**: Verify CRDT convergence under concurrent writes
- **Nodes**: 3 nodes
- **Duration**: ~2-3 minutes
- **Tests**: 
  - Multiple nodes writing to same key simultaneously
  - Last-Write-Wins conflict resolution
  - State convergence validation
  - CRDT consistency guarantees
  - Conflict resolution under load
- **Use Case**: CRDT correctness validation, conflict resolution testing

---

## 🎯 **Workflow Selection Guide**

### **For Development:**
```bash
# Quick validation during development
merobox bootstrap run workflows/bootstrap-short.yml

# Basic functionality and performance test
merobox bootstrap run workflows/kv-store-simple.yml
```

### **For Performance Testing:**
```bash
# Phase 2 optimizations (batch processing, lightweight deltas)
merobox bootstrap run workflows/phase2-performance-test.yml

# Phase 3 optimizations (direct P2P, binary protocol)
merobox bootstrap run workflows/phase3-performance-test.yml
```

### **For Convergence Testing:**
```bash
# CRDT convergence validation
merobox bootstrap run workflows/convergence-test.yml
```

### **For Production Validation:**
```bash
# Complete production readiness test (comprehensive)
merobox bootstrap run workflows/bootstrap.yml
```

---

## 📊 **Expected Results**

### **Performance Targets:**
- **Propagation Time**: 0s (immediate) for most operations
- **Convergence Time**: <200ms for CRDT convergence
- **Scale**: Stable performance up to 8+ nodes
- **Reliability**: 100% convergence under all conditions

### **Success Criteria:**
- ✅ All nodes see identical final state
- ✅ No errors or timeouts
- ✅ Perfect CRDT convergence
- ✅ Stable performance under load

---

## 🔧 **Configuration**

### **Common Settings:**
- **Image**: `localhost:5001/merod:latest` (local development)
- **Chain ID**: `testnet-1`
- **Timeout**: 120-180 seconds (depending on workflow)
- **Stop All Nodes**: `true` (cleanup after test)

### **Node Counts:**
- **Small Tests**: 3 nodes (quick validation)
- **Medium Tests**: 5 nodes (performance testing)
- **Large Tests**: 8 nodes (scale testing)

---

## 🚨 **Troubleshooting**

### **Common Issues:**
1. **Timeout Errors**: Increase `wait_timeout` in workflow
2. **Node Creation Failures**: Check Docker registry and image availability
3. **Sync Issues**: Increase wait times between steps
4. **Performance Degradation**: Check system resources and network

### **Debug Commands:**
```bash
# Check node status
docker ps | grep calimero-node

# View node logs
docker logs calimero-node-1

# Check network connectivity
docker exec calimero-node-1 ping calimero-node-2
```

---

## 📈 **Performance Metrics**

### **Key Measurements:**
- **Propagation Latency**: Time for state changes to reach all nodes
- **Convergence Time**: Time for all nodes to reach consistent state
- **Throughput**: Operations per second under load
- **Memory Usage**: RAM consumption during operations
- **CPU Usage**: Processor utilization during sync

### **Baseline Performance:**
- **Propagation**: 0s (immediate)
- **Convergence**: <200ms
- **Scale**: 8+ nodes stable
- **Reliability**: 100% convergence

---

## 🧹 **Workflow Cleanup Summary**

### **Removed Redundant Workflows:**
- ❌ `kv-store-benchmark.yml` - Redundant with `bootstrap.yml`
- ❌ `performance-test.yml` - Redundant with `kv-store-simple.yml`
- ❌ `kv-store-timing-benchmark.yml` - Functionality covered by other workflows

### **Kept Essential Workflows:**
- ✅ `bootstrap.yml` - Comprehensive production test
- ✅ `bootstrap-short.yml` - Quick development test
- ✅ `kv-store-simple.yml` - Basic functionality + performance
- ✅ `phase2-performance-test.yml` - Phase 2 optimization validation
- ✅ `phase3-performance-test.yml` - Phase 3 optimization validation
- ✅ `convergence-test.yml` - CRDT convergence validation

**Result**: Reduced from 9 workflows to 6 essential workflows while maintaining full test coverage.

---

*Last Updated: Performance Optimization Branch*  
*Status: ✅ Cleaned up and optimized workflow set*
