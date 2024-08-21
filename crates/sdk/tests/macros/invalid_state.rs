#[app::state]
#[app::state()]
#[app::state("")]
#[app::state(emits)]
#[app::state(emits none)]
#[app::state(emits = )]
#[app::state(emits = "")]
#[app::state(emits = Event alt)]
#[app::state(emits = Event<'a>)]
#[app::state(emits = Event<&str>)]
#[app::state(emits = for<>)]
#[app::state(emits = for<'a>)]
#[app::state(emits = for<'a> "")]
#[app::state(emits = for<'a> Event)]
#[app::state(emits = for<'a> Event<'a>)]
#[app::state(emits = for<'a> Event<'a> alt)]
#[derive(BorshDeserialize, BorshSerialize, Default)]
#[borsh(crate = "calimero_sdk::borsh")]
struct MyType {
    items: HashMap<String, String>,
}

#[app::event]
pub enum Event<'a> {
    Inserted { key: &'a str, value: &'a str },
    Updated { key: &'a str, value: &'a str },
    Removed { key: &'a str },
    Cleared,
}
