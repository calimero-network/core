# Team Metrics - With Custom Implementation

**Example: Manual `Mergeable` implementation for full control**

## The Code

```rust
pub struct TeamStats {
    pub wins: Counter,
    pub losses: Counter,
    pub draws: Counter,
}

impl Mergeable for TeamStats {
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        // You have full control!
        // - Add logging
        // - Skip fields conditionally
        // - Apply business rules
        // - Validate invariants
        
        self.wins.merge(&other.wins)?;
        self.losses.merge(&other.losses)?;
        self.draws.merge(&other.draws)?;
        
        // Example: Custom validation
        // if self.wins.value()? > 1000 {
        //     return Err(MergeError::InvalidValue("Too many wins!".into()));
        // }
        
        Ok(())
    }
}
```

## Why This Approach?

✅ **Full control** - Custom merge logic  
✅ **Flexible** - Add logging, validation, etc.  
✅ **Advanced** - For complex scenarios  

## When to Use

- Need custom merge behavior
- Want to add logging/validation
- Need to skip certain fields
- Business rules to apply

## Compare With

See `apps/team-metrics-macro` for the simple derive approach (recommended for most cases).

## Build & Test

```bash
./build.sh
cd ../../e2e-tests
cargo run -- --protocol near --test team-metrics-test
```

**Note:** Both apps have the same functionality - only the implementation differs!

