use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Once;

use super::{HostError, PanicContext, VMLogic};

thread_local! {
    // https://open.spotify.com/track/7DPUuTaTZCtQ6o4Xx00qzT
    static HOOKER: Once = Once::new();
    static PAYLOAD: RefCell<Option<String>> = RefCell::new(None);
    static HOST_CTX: AtomicBool = AtomicBool::new(false);
}

impl<'a> VMLogic<'a> {
    pub fn imports(&mut self, store: &mut wasmer::Store) -> wasmer::Imports {
        imports! {
            store;
            logic: self;

            fn panic();
            fn panic_utf8(len: u64, ptr: u64);

            // todo! custom memory injection
            fn register_len(register_id: u64) -> u64;
            fn read_register(register_id: u64, ptr: u64);

            fn input(register_id: u64);
            fn value_return(value_len: u64, value_ptr: u64);
            fn log_utf8(len: u64, ptr: u64);

            fn storage_write(
                key_len: u64,
                key_ptr: u64,
                value_len: u64,
                value_ptr: u64,
                register_id: u64,
            ) -> u32;
            fn storage_read(key_len: u64, key_ptr: u64, register_id: u64) -> u32;
        }
    }
}

macro_rules! _imports {
    ($store:ident; logic: $logic:ident; $(fn $func:ident($($arg:ident: $arg_ty:ty),*$(,)?) $(-> $returns:ty)?;)*) => {
        {
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
                                Some(location) => format!("panicked at {}: {}", location, message),
                                None => format!("fatal: panicked at unknown location: {}", message),
                            });
                        });

                        prev_hook(info);
                    }));
                });
            });

            $(
                #[allow(unused_parens)]
                fn $func(
                    mut env: wasmer::FunctionEnvMut<fragile::Fragile<*mut ()>>,
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
                    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        let (data, store) = env.data_and_store_mut();
                        let data = unsafe { &mut *(*data.get_mut() as *mut VMLogic) };

                        data.host_functions(store).$func($($arg),*)
                    })).unwrap_or_else(|_| Err(HostError::Panic {
                        context: PanicContext::Host,
                        message: PAYLOAD.with(|payload| {
                            payload.borrow_mut().take().unwrap_or_else(|| "<no message>".to_string())
                        })
                    }.into()));
                    HOST_CTX.with(|ctx| ctx.store(false, Ordering::Relaxed));

                    #[cfg(feature = "host-traces")] {
                        #[allow(unused_mut, unused_assignments)]
                        let mut return_ty = "()".to_string();
                        $( return_ty = stringify!($returns).to_string(); )?
                        println!(
                            " ⇲ {}(..) -> {} = {res:?}",
                            stringify!($func).fg_rgb::<166, 226, 46>(),
                            return_ty.fg_rgb::<102, 217, 239>()
                        );
                    }

                    res.map_err(|err| wasmer::RuntimeError::user(Box::new(err)))
                }
            )*

            let env = wasmer::FunctionEnv::new(&mut store, fragile::Fragile::new(logic as *mut _ as *mut ()));

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
