use core::cell::RefCell;
use core::sync::atomic::{AtomicBool, Ordering};
use std::sync::Once;

use wasmer::{Imports, Store};

use super::{HostError, Location, PanicContext, VMLogic};

thread_local! {
    // https://open.spotify.com/track/7DPUuTaTZCtQ6o4Xx00qzT
    static HOOKER: Once = const { Once::new() };
    static PAYLOAD: RefCell<Option<(String, Location)>> = const { RefCell::new(None) };
    static HOST_CTX: AtomicBool = const { AtomicBool::new(false) };
}

impl VMLogic<'_> {
    pub fn imports(&mut self, store: &mut Store) -> Imports {
        imports! {
            store;
            logic: self;

            fn panic(location_ptr: u64);
            fn panic_utf8(msg_ptr: u64, file_ptr: u64);

            // todo! custom memory injection
            fn register_len(register_id: u64) -> u64;
            fn read_register(register_id: u64, register_ptr: u64) -> u32;

            fn context_id(register_id: u64);
            fn executor_id(register_id: u64);

            fn input(register_id: u64);
            fn value_return(value_ptr: u64);
            fn log_utf8(log_ptr: u64);
            fn emit(event_ptr: u64);
            fn emit_with_handler(event_ptr: u64, handler_ptr: u64);
            fn xcall(xcall_ptr: u64);

            fn commit(root_hash_ptr: u64, artifact_ptr: u64);

            fn storage_write(
                key_ptr: u64,
                value_ptr: u64,
                register_id: u64,
            ) -> u32;
            fn storage_read(key_ptr: u64, register_id: u64) -> u32;
            fn storage_remove(key_ptr: u64, register_id: u64) -> u32;

            fn fetch(
                url_ptr: u64,
                method_ptr: u64,
                headers_ptr: u64,
                body_ptr: u64,
                register_id: u64,
            ) -> u32;

            fn random_bytes(ptr: u64);
            fn time_now(ptr: u64);

            fn send_proposal(actions_ptr: u64, id_ptr: u64);
            fn approve_proposal(approval_ptr: u64);

            fn blob_create() -> u64;
            fn blob_write(fd: u64, data_ptr: u64) -> u64;
            fn blob_close(fd: u64, blob_id_ptr: u64) -> u32;
            fn blob_open(blob_id_ptr: u64) -> u64;
            fn blob_read(fd: u64, data_ptr: u64) -> u64;
            fn blob_announce_to_context(blob_id_ptr: u64, context_id_ptr: u64) -> u32;
        }
    }
}

macro_rules! _imports {
    ($store:ident; logic: $logic:ident; $(fn $func:ident($($arg:ident: $arg_ty:ty),*$(,)?) $(-> $returns:ty)?;)*) => {
        {
            $(
                #[expect(clippy::allow_attributes, reason = "Needed for the macro")]
                #[allow(unused_parens, reason = "Needed for the macro")]
                fn $func(
                    mut env: wasmer::FunctionEnvMut<'_, fragile::Fragile<*mut ()>>,
                    $($arg: $arg_ty),*
                ) -> Result<($( $returns )?), wasmer::RuntimeError> {
                    #[cfg(feature = "host-traces")]
                    use owo_colors::OwoColorize;

                    #[cfg(feature = "host-traces")]
                    {
                        let params: &[String] = &[$(
                            format!(
                                "{}: {} = {}",
                                stringify!($arg).fg_rgb::<253, 151, 31>(),
                                stringify!($arg_ty).fg_rgb::<102, 217, 239>(),
                                $arg.fg_rgb::<190, 132, 255>()
                            )
                        ),*][..];

                        let decorator = format!(
                            "{} {}({})",
                            "fn".fg_rgb::<102, 217, 239>(),
                            stringify!($func).fg_rgb::<166, 226, 46>(),
                            params.join(", ")
                        );

                        println!("{}", decorator);
                    };

                    HOST_CTX.with(|ctx| ctx.store(true, Ordering::Relaxed));
                    let res = std::panic::catch_unwind(core::panic::AssertUnwindSafe(|| {
                        let (data, store) = env.data_and_store_mut();
                        let data = unsafe { &mut *(*data.get_mut()).cast::<VMLogic<'_>>() };

                        data.host_functions(store).$func($($arg),*)
                    })).unwrap_or_else(|_| {
                        let (message, location) = PAYLOAD.with(|payload| {
                            payload.borrow_mut().take().unwrap_or_else(|| ("<no message>".to_owned(), Location::Unknown))
                        });

                        Err(HostError::Panic {
                            context: PanicContext::Host,
                            message,
                            location,
                        }.into())
                    });
                    HOST_CTX.with(|ctx| ctx.store(false, Ordering::Relaxed));

                    #[cfg(feature = "host-traces")]
                    {
                        #[allow(unused_mut, unused_assignments)]
                        let mut return_ty = "()";
                        $( return_ty = stringify!($returns); )?
                        println!(
                            " ⇲ {}(..) -> {} = {res:?}",
                            stringify!($func).fg_rgb::<166, 226, 46>(),
                            return_ty.fg_rgb::<102, 217, 239>()
                        );
                    }

                    res.map_err(|err| wasmer::RuntimeError::user(Box::new(err)))
                }
            )*

            let mut store = $store;
            let logic = $logic;

            HOOKER.with(|hooker| {
                hooker.call_once(|| {
                    let prev_hook = std::panic::take_hook();
                    std::panic::set_hook(Box::new(move |info| {
                        if !HOST_CTX.with(|ctx| ctx.load(Ordering::Relaxed)) {
                            return prev_hook(info);
                        }
                        PAYLOAD.with(|payload| {
                            let message = match info.payload().downcast_ref::<&'static str>() {
                                Some(message) => *message,
                                None => match info.payload().downcast_ref::<String>() {
                                    Some(message) => &**message,
                                    None => "<no message>",
                                },
                            };

                            *payload.borrow_mut() = Some(match info.location() {
                                Some(location) => (message.to_owned(), Location::from(location)),
                                None => (message.to_owned(), Location::Unknown),
                            });
                        });

                        prev_hook(info);
                    }));
                });
            });

            let env = wasmer::FunctionEnv::new(&mut store, fragile::Fragile::new(core::ptr::from_mut(logic).cast::<()>()));

            wasmer::imports! {
                "env" => {
                    $(
                        stringify!($func) => wasmer::Function::new_typed_with_env(&mut store, &env, $func),
                    )*
                }
            }
        }
    };
}

use _imports as imports;
