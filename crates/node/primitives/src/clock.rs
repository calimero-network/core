use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

/// Hybrid Logical Clock (HLC) implementation
/// 
/// HLC combines physical time (pt) with logical time (lt) to provide
/// monotonic timestamps that are both causally consistent and wall-clock
/// synchronized. This eliminates clock skew issues while maintaining
/// simple Last-Write-Wins semantics.
/// 
/// Structure:
/// - pt: Physical time (wall clock) in nanoseconds since Unix epoch
/// - lt: Logical time counter for events within the same physical time
/// - node_id: Unique identifier for the node generating the timestamp
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, BorshSerialize, BorshDeserialize, Serialize, Deserialize)]
pub struct Hlc {
    /// Physical time in nanoseconds since Unix epoch
    pub pt: u64,
    /// Logical time counter for events within same physical time
    pub lt: u64,
    /// Node identifier (32 bytes)
    pub node_id: [u8; 32],
}

impl Hlc {
    /// Create a new HLC instance
    pub fn new(node_id: [u8; 32]) -> Self {
        let pt = Self::current_physical_time();
        Self {
            pt,
            lt: 0,
            node_id,
        }
    }

    /// Get current physical time in nanoseconds
    fn current_physical_time() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64
    }

    /// Generate a new HLC timestamp
    /// 
    /// This method ensures monotonicity by:
    /// 1. Getting current physical time
    /// 2. If current time > our physical time, reset logical counter
    /// 3. If current time == our physical time, increment logical counter
    /// 4. If current time < our physical time, increment logical counter
    pub fn now(&mut self) -> Self {
        let current_pt = Self::current_physical_time();
        
        if current_pt > self.pt {
            // Physical time has advanced, reset logical counter
            self.pt = current_pt;
            self.lt = 0;
        } else {
            // Same or older physical time, increment logical counter
            self.lt += 1;
        }
        
        *self
    }

    /// Update HLC with an incoming timestamp
    /// 
    /// This ensures causal ordering by taking the maximum of:
    /// 1. Current physical time
    /// 2. Incoming physical time
    /// 3. Current logical time + 1
    /// 4. Incoming logical time + 1
    pub fn update(&mut self, incoming: &Hlc) -> Self {
        let current_pt = Self::current_physical_time();
        
        let new_pt = current_pt.max(incoming.pt);
        let new_lt = if new_pt == self.pt && new_pt == incoming.pt {
            // All timestamps have same physical time
            self.lt.max(incoming.lt) + 1
        } else if new_pt == self.pt {
            // Our physical time is the maximum
            self.lt + 1
        } else if new_pt == incoming.pt {
            // Incoming physical time is the maximum
            incoming.lt + 1
        } else {
            // Physical time has advanced
            0
        };
        
        self.pt = new_pt;
        self.lt = new_lt;
        
        *self
    }

    /// Check if this HLC is newer than another
    /// 
    /// Returns true if this timestamp is causally newer than the other.
    /// This is used for Last-Write-Wins conflict resolution.
    pub fn is_newer_than(&self, other: &Hlc) -> bool {
        if self.pt > other.pt {
            true
        } else if self.pt < other.pt {
            false
        } else {
            // Same physical time, compare logical time
            self.lt > other.lt
        }
    }

    /// Check if this HLC is newer than or equal to another
    pub fn is_newer_than_or_equal(&self, other: &Hlc) -> bool {
        if self.pt > other.pt {
            true
        } else if self.pt < other.pt {
            false
        } else {
            // Same physical time, compare logical time
            self.lt >= other.lt
        }
    }

    /// Convert to u64 for backward compatibility
    /// 
    /// This is a lossy conversion that prioritizes physical time
    /// and uses logical time as a tiebreaker in the lower bits.
    /// 
    /// Convert to u64 for backward compatibility
    /// 
    /// Returns the physical time in nanoseconds directly
    pub fn to_u64(&self) -> u64 {
        self.pt
    }

    /// Create from u64 (for backward compatibility)
    /// 
    /// This is a lossy conversion that reconstructs an approximation
    /// of the original HLC.
    pub fn from_u64(value: u64, node_id: [u8; 32]) -> Self {
        // For backward compatibility, treat the u64 as a simple timestamp
        // Use the value directly as physical time in nanoseconds
        let pt = value;
        let lt = 0;
        Self { pt, lt, node_id }
    }
}

impl Default for Hlc {
    fn default() -> Self {
        Self::new([0; 32])
    }
}

/// Helper function to compare two HLCs
/// 
/// Returns true if `a` is newer than `b`
pub fn hlc_is_newer(a: &Hlc, b: &Hlc) -> bool {
    a.is_newer_than(b)
}

/// Helper function to compare two HLCs
/// 
/// Returns true if `a` is newer than or equal to `b`
pub fn hlc_is_newer_or_equal(a: &Hlc, b: &Hlc) -> bool {
    a.is_newer_than_or_equal(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hlc_creation() {
        let node_id = [1u8; 32];
        let hlc = Hlc::new(node_id);
        
        assert_eq!(hlc.node_id, node_id);
        assert!(hlc.pt > 0);
        assert_eq!(hlc.lt, 0);
    }

    #[test]
    fn test_hlc_monotonicity() {
        let mut hlc = Hlc::new([1u8; 32]);
        let first = hlc.now();
        
        // Small delay to ensure different timestamps
        std::thread::sleep(std::time::Duration::from_millis(1));
        
        let second = hlc.now();
        assert!(second.is_newer_than(&first));
    }

    #[test]
    fn test_hlc_update() {
        let mut hlc1 = Hlc::new([1u8; 32]);
        let mut hlc2 = Hlc::new([2u8; 32]);
        
        let ts1 = hlc1.now();
        let ts2 = hlc2.now();
        
        // Update hlc1 with ts2
        let updated = hlc1.update(&ts2);
        assert!(updated.is_newer_than(&ts1));
        assert!(updated.is_newer_than(&ts2));
    }

    #[test]
    fn test_hlc_comparison() {
        let mut hlc = Hlc::new([1u8; 32]);
        let ts1 = hlc.now();
        let ts2 = hlc.now();
        
        assert!(ts2.is_newer_than(&ts1));
        assert!(!ts1.is_newer_than(&ts2));
        assert!(ts2.is_newer_than_or_equal(&ts1));
        assert!(ts1.is_newer_than_or_equal(&ts1));
    }

    #[test]
    fn test_hlc_u64_conversion() {
        let node_id = [1u8; 32];
        let mut hlc = Hlc::new(node_id);
        let original = hlc.now();
        
        let u64_value = original.to_u64();
        let reconstructed = Hlc::from_u64(u64_value, node_id);
        
        // For backward compatibility, the conversion should preserve the node_id
        assert_eq!(reconstructed.node_id, original.node_id);
        
        // The reconstructed physical time should be exactly the same as original
        // since we're using the u64 value directly as physical time
        assert_eq!(reconstructed.pt, original.pt);
        
        // The reconstructed logical time should be 0 (reset for backward compatibility)
        assert_eq!(reconstructed.lt, 0);
        
        // The conversion should preserve the basic ordering relationship
        // Since physical time is preserved, but logical time is reset to 0,
        // the reconstructed should be newer or equal only if original has logical time 0
        if original.lt == 0 {
            assert!(reconstructed.is_newer_than_or_equal(&original));
        } else {
            // If original has logical time > 0, reconstructed will be older due to logical time reset
            assert!(!reconstructed.is_newer_than(&original));
        }
    }

    #[test]
    fn test_hlc_helper_functions() {
        let mut hlc = Hlc::new([1u8; 32]);
        let ts1 = hlc.now();
        let ts2 = hlc.now();
        
        assert!(hlc_is_newer(&ts2, &ts1));
        assert!(!hlc_is_newer(&ts1, &ts2));
        assert!(hlc_is_newer_or_equal(&ts2, &ts1));
        assert!(hlc_is_newer_or_equal(&ts1, &ts1));
    }
}
