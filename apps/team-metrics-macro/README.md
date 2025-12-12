# Team Metrics - With Derive Macro

**Example: Using `#[derive(Mergeable)]` for zero boilerplate**

## The Code

```rust
#[derive(Mergeable, BorshSerialize, BorshDeserialize)]
pub struct TeamStats {
    pub wins: Counter,
    pub losses: Counter,
    pub draws: Counter,
}
// That's it! No manual impl needed! ✨
```

**The macro auto-generates:**
```rust
impl Mergeable for TeamStats {
    fn merge(&mut self, other: &Self) -> Result<(), MergeError> {
        self.wins.merge(&other.wins)?;
        self.losses.merge(&other.losses)?;
        self.draws.merge(&other.draws)?;
        Ok(())
    }
}
```

## Why This Approach?

✅ **Simplest** - Just add `#[derive(Mergeable)]`  
✅ **Zero boilerplate** - No manual impl  
✅ **Correct by default** - Macro generates proper merge  
✅ **Recommended** - For most use cases  

## When to Use

- ✅ All fields are CRDTs
- ✅ Standard merge behavior is what you want
- ✅ No custom logic needed

## Compare With

See `apps/team-metrics-custom` for the manual approach when you need custom logic.

## Build & Test

End-to-end tests are automatically run via the `merobox-workflows.yml` GitHub Actions workflow, which executes workflows defined in `team-metrics-macro/workflows/*.yml`.

To build locally:

```bash
./build.sh
```

To run workflow tests locally using merobox:

```bash
./build.sh
merobox bootstrap run workflows/team-metrics-test.yml \
  --no-docker \
  --binary-path ../../target/debug/merod \
  --e2e-mode \
  --near-devnet
```
