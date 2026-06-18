//! `#[app::state]` / `#[app::logic]` compose with standard Rust attributes
//! (`#[derive]`, doc comments, `#[repr]`, `#[cfg]`, `#[allow]`, `#[must_use]`,
//! `#[inline]`, `#[deprecated]`).
//!
//! Each scenario keeps its original fields; bare value types are wrapped in
//! `LwwRegister<T>` (the CRDT form), which derefs to `T` so the `&self.field`
//! accessors compile unchanged.

use calimero_sdk::app;
use calimero_storage::collections::LwwRegister;

// Test combining app::state with a derive attribute
#[app::state]
#[derive(Clone)]
struct StateWithDerive {
    value: LwwRegister<String>,
}

#[app::logic]
impl StateWithDerive {
    #[app::init]
    pub fn init() -> StateWithDerive {
        StateWithDerive {
            value: LwwRegister::new(String::new()),
        }
    }

    pub fn get_value(&self) -> &str {
        &self.value
    }

    pub fn clone_self(&self) -> Self {
        self.clone()
    }
}

// Test combining app::state with multiple derive attributes
#[app::state]
#[derive(Clone, Default)]
struct StateWithMultipleDerive {
    count: LwwRegister<u64>,
    items: LwwRegister<Vec<String>>,
}

#[app::logic]
impl StateWithMultipleDerive {
    #[app::init]
    pub fn init() -> Self {
        Self::default()
    }

    pub fn get_count(&self) -> u64 {
        *self.count
    }
}

// Test doc attributes on state
/// This is a documented state struct
#[app::state]
struct DocumentedState {
    /// The main data field
    data: LwwRegister<String>,
}

#[app::logic]
impl DocumentedState {
    #[app::init]
    pub fn init() -> DocumentedState {
        DocumentedState {
            data: LwwRegister::new(String::new()),
        }
    }

    /// Gets the data
    pub fn get_data(&self) -> &str {
        &self.data
    }

    /// Sets the data
    pub fn set_data(&mut self, value: String) {
        self.data.set(value);
    }
}

// Test allow/deny attributes on logic methods
#[app::state]
struct StateWithLintAttributes;

#[app::logic]
impl StateWithLintAttributes {
    #[app::init]
    pub fn init() -> StateWithLintAttributes {
        StateWithLintAttributes
    }

    #[allow(unused_variables)]
    pub fn with_allow(&self, unused: String) {}

    #[allow(clippy::needless_return)]
    pub fn with_clippy_allow(&self) -> u64 {
        return 42;
    }
}

// Test repr attributes don't interfere
#[repr(C)]
#[app::state]
struct ReprCState {
    x: LwwRegister<i32>,
    y: LwwRegister<i32>,
}

#[app::logic]
impl ReprCState {
    #[app::init]
    pub fn init() -> ReprCState {
        ReprCState {
            x: LwwRegister::new(0),
            y: LwwRegister::new(0),
        }
    }

    pub fn get_x(&self) -> i32 {
        *self.x
    }

    pub fn get_y(&self) -> i32 {
        *self.y
    }
}

// Test cfg attributes on a logic method. trybuild compiles each fixture as a
// plain binary, so `#[cfg(test)]` is *false* here — `get_conditional` is
// compiled out, which is itself the attribute/macro interplay under test.
#[app::state]
struct ConditionalState {
    always_present: LwwRegister<u64>,
}

#[app::logic]
impl ConditionalState {
    #[app::init]
    pub fn init() -> ConditionalState {
        ConditionalState {
            always_present: LwwRegister::new(0),
        }
    }

    pub fn get_always_present(&self) -> u64 {
        *self.always_present
    }

    #[cfg(test)]
    pub fn get_conditional(&self) -> u64 {
        0
    }
}

// Test must_use attribute on methods
#[app::state]
struct MustUseState {
    value: LwwRegister<u64>,
}

#[app::logic]
impl MustUseState {
    #[app::init]
    pub fn init() -> MustUseState {
        MustUseState {
            value: LwwRegister::new(0),
        }
    }

    #[must_use]
    pub fn compute(&self) -> u64 {
        *self.value * 2
    }
}

// Test inline attributes
#[app::state]
struct InlineState {
    data: LwwRegister<String>,
}

#[app::logic]
impl InlineState {
    #[app::init]
    pub fn init() -> InlineState {
        InlineState {
            data: LwwRegister::new(String::new()),
        }
    }

    #[inline]
    pub fn get_inline(&self) -> &str {
        &self.data
    }

    #[inline(always)]
    pub fn get_inline_always(&self) -> &str {
        &self.data
    }
}

// Test deprecated attribute
#[app::state]
struct DeprecatedMethodState {
    value: LwwRegister<String>,
}

#[app::logic]
impl DeprecatedMethodState {
    #[app::init]
    pub fn init() -> DeprecatedMethodState {
        DeprecatedMethodState {
            value: LwwRegister::new(String::new()),
        }
    }

    #[deprecated(note = "Use get_new_value instead")]
    pub fn get_old_value(&self) -> &str {
        &self.value
    }

    pub fn get_new_value(&self) -> &str {
        &self.value
    }
}

// Test that #[app::view] (the read-only intent marker) compiles, on its own
// and combined with other attributes like #[must_use].
#[app::state]
struct ViewState {
    value: String,
}

#[app::logic]
impl ViewState {
    #[app::view]
    pub fn get_value(&self) -> &str {
        &self.value
    }

    #[app::view]
    #[must_use]
    pub fn value_len(&self) -> usize {
        self.value.len()
    }

    pub fn set_value(&mut self, value: String) {
        self.value = value;
    }
}

fn main() {}
