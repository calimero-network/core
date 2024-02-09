use super::VMLogic;

impl<'a> VMLogic<'a> {
    pub fn imports(&mut self, store: &mut wasmer::Store) -> wasmer::Imports {
        imports! {
            store;
            logic: self;

            // todo! custom memory injection
            fn read_register(register_id: u64, ptr: u64);
            fn register_len(register_id: u64) -> u64;
            // --
            fn input(register_id: u64);
            // --
            fn panic();
            fn panic_utf8(len: u64, ptr: u64);
            fn value_return(value_len: u64, value_ptr: u64);
            fn log_utf8(len: u64, ptr: u64);
            // --
            fn storage_write(
                key_len: u64,
                key_ptr: u64,
                value_len: u64,
                value_ptr: u64,
                register_id: u64,
            ) -> u64;
            fn storage_read(key_len: u64, key_ptr: u64, register_id: u64) -> u64;
        }
    }
}

macro_rules! _imports {
    ($store:ident; logic: $self:ident; $(fn $func:ident($($arg:ident: $arg_ty:ty),*$(,)?) $(-> $returns:ty)?;)*) => {
        {
            let mut store = $store;
            let logic = $self;

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
                                ($arg).fg_rgb::<190, 132, 255>()
                            )),*][..];

                        let decorator = format!(
                            "{} {}({})",
                            "fn".fg_rgb::<102, 217, 239>(),
                            stringify!($func).fg_rgb::<166, 226, 46>(),
                            params.join(", ")
                        );

                        println!("{}", decorator);
                    };

                    let (data, store) = env.data_and_store_mut();
                    let data = unsafe { &mut *(*data.get_mut() as *mut VMLogic) };

                    let res = data
                        .host_functions(store)
                        .$func($($arg),*)
                        .map_err(|err| wasmer::RuntimeError::user(Box::new(err)));

                    #[cfg(feature = "host-traces")]
                    println!(" ⇲ {}(..) -> {:?}", stringify!($func).fg_rgb::<166, 226, 46>(), res);

                    res
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
