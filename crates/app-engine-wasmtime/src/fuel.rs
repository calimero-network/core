//! Host calls pay their fuel where they happen (C7, `calimero_app_host`'s fuel table).
//!
//! Every host function that does work is linked through a wrapper that holds the store: it
//! takes [`FUEL_HOST_CALL`] out of the run's fuel before the call looks at anything, runs
//! it, then takes what the host core says the call owes (`HostCore::take_host_fuel`: the
//! bytes it was handed, its results). Stream writes and reads pay per byte here. A run whose
//! fuel cannot pay ends `Trap::OutOfFuel` at that call, as a guest that runs out of its own
//! fuel does, so the guest never runs on fuel its host calls already owe. A charge that runs
//! out wins over the call's own refusal: a denied call that could pay is `HostDenied`.
//!
//! The wrappers shadow the plain functions `bindgen!` linked (`Linker::allow_shadowing`);
//! resource drops stay plain, and what they owe is paid at the next call or at the end.

use calimero_app_host::{FUEL_HOST_CALL, FUEL_PER_BYTE};
use wasmtime::component::{Linker, LinkerInstance, Resource, ResourceTable};
use wasmtime::{StoreContextMut, Trap};
use wasmtime_wasi::p2::bindings::sync::io::streams;
use wasmtime_wasi::p2::{DynInputStream, DynOutputStream, StreamResult};

use crate::{StoreData, World};

/// Takes `fuel` out of the run's fuel, or ends the run out of fuel when it cannot pay.
pub(crate) fn pay(store: &mut StoreContextMut<'_, StoreData>, fuel: u64) -> wasmtime::Result<()> {
    if fuel == 0 {
        return Ok(());
    }
    let left = store.get_fuel()?;
    if fuel > left {
        store.set_fuel(0)?;
        return Err(wasmtime::Error::new(Trap::OutOfFuel));
    }
    store.set_fuel(left - fuel)
}

/// Pays what the host core says the run's calls owe.
pub(crate) fn settle(store: &mut StoreContextMut<'_, StoreData>) -> wasmtime::Result<()> {
    let owed = store.data_mut().host.take_host_fuel();
    pay(store, owed)
}

fn bytes(n: u64) -> u64 {
    n.saturating_mul(FUEL_PER_BYTE)
}

/// Links `$name` on `$inst` as a call of `$body` that pays the base before and what the host
/// core says it owes after. `-> ()` for a function with no result.
macro_rules! charged {
    ($inst:ident, $name:literal, |$d:ident $(, $a:ident : $t:ty)*| -> () $body:expr) => {
        $inst.func_wrap(
            $name,
            |mut store: ::wasmtime::StoreContextMut<'_, $crate::StoreData>, ($($a,)*): ($($t,)*)| -> ::wasmtime::Result<()> {
                $crate::fuel::pay(&mut store, ::calimero_app_host::FUEL_HOST_CALL)?;
                let out = {
                    let $d = store.data_mut();
                    $body
                };
                $crate::fuel::settle(&mut store)?;
                out
            },
        )?;
    };
    ($inst:ident, $name:literal, |$d:ident $(, $a:ident : $t:ty)*| $body:expr) => {
        $inst.func_wrap(
            $name,
            |mut store: ::wasmtime::StoreContextMut<'_, $crate::StoreData>, ($($a,)*): ($($t,)*)| {
                $crate::fuel::pay(&mut store, ::calimero_app_host::FUEL_HOST_CALL)?;
                let out = {
                    let $d = store.data_mut();
                    $body
                };
                $crate::fuel::settle(&mut store)?;
                Ok((out?,))
            },
        )?;
    };
}
pub(crate) use charged;

/// The `calimero:app` instance `interface` at this build's version.
pub(crate) fn calimero<'a>(
    linker: &'a mut Linker<StoreData>,
    interface: &str,
) -> wasmtime::Result<LinkerInstance<'a, StoreData>> {
    let version = calimero_app_wit::WORLD
        .rsplit_once('@')
        .map_or("", |(_, v)| v);
    linker.instance(&format!("calimero:app/{interface}@{version}"))
}

/// Shadows every host function that does work with its charged version.
pub(crate) fn charge_calls(linker: &mut Linker<StoreData>, world: World) -> wasmtime::Result<()> {
    match world {
        World::App => crate::bindings::charge_calls(linker)?,
        World::Compute => crate::compute::charge_calls(linker)?,
    }
    crate::bindings::charge_files(linker)?;
    charge_wasi(linker)
}

/// WASI: randomness through the host core, and stream writes and reads per byte (stdout and
/// stderr, also once the log is full and the bytes are dropped, and `wasi:http` bodies).
fn charge_wasi(linker: &mut Linker<StoreData>) -> wasmtime::Result<()> {
    use wasmtime_wasi::p2::bindings::random::{insecure_seed, random};
    {
        let mut i = linker.instance("wasi:random/random@0.2.12")?;
        charged!(i, "get-random-bytes", |d, len: u64| {
            random::Host::get_random_bytes(d, len)
        });
        charged!(i, "get-random-u64", |d| random::Host::get_random_u64(d));
    }
    {
        let mut i = linker.instance("wasi:random/insecure-seed@0.2.12")?;
        charged!(i, "insecure-seed", |d| insecure_seed::Host::insecure_seed(
            d
        ));
    }
    let mut i = linker.instance("wasi:io/streams@0.2.12")?;
    i.func_wrap(
        "[method]output-stream.write",
        |mut s: StoreContextMut<'_, StoreData>,
         (this, contents): (Resource<DynOutputStream>, Vec<u8>)| {
            pay(
                &mut s,
                FUEL_HOST_CALL.saturating_add(bytes(contents.len() as u64)),
            )?;
            let t = &mut s.data_mut().table;
            let r = streams::HostOutputStream::write(t, this, contents);
            Ok((stream(t, r)?,))
        },
    )?;
    i.func_wrap(
        "[method]output-stream.blocking-write-and-flush",
        |mut s: StoreContextMut<'_, StoreData>,
         (this, contents): (Resource<DynOutputStream>, Vec<u8>)| {
            pay(
                &mut s,
                FUEL_HOST_CALL.saturating_add(bytes(contents.len() as u64)),
            )?;
            let t = &mut s.data_mut().table;
            let r = streams::HostOutputStream::blocking_write_and_flush(t, this, contents);
            Ok((stream(t, r)?,))
        },
    )?;
    i.func_wrap(
        "[method]output-stream.write-zeroes",
        |mut s: StoreContextMut<'_, StoreData>, (this, len): (Resource<DynOutputStream>, u64)| {
            pay(&mut s, FUEL_HOST_CALL.saturating_add(bytes(len)))?;
            let t = &mut s.data_mut().table;
            let r = streams::HostOutputStream::write_zeroes(t, this, len);
            Ok((stream(t, r)?,))
        },
    )?;
    i.func_wrap(
        "[method]output-stream.blocking-write-zeroes-and-flush",
        |mut s: StoreContextMut<'_, StoreData>, (this, len): (Resource<DynOutputStream>, u64)| {
            pay(&mut s, FUEL_HOST_CALL.saturating_add(bytes(len)))?;
            let t = &mut s.data_mut().table;
            let r = streams::HostOutputStream::blocking_write_zeroes_and_flush(t, this, len);
            Ok((stream(t, r)?,))
        },
    )?;
    i.func_wrap(
        "[method]output-stream.splice",
        |mut s: StoreContextMut<'_, StoreData>,
         (this, src, len): (Resource<DynOutputStream>, Resource<DynInputStream>, u64)| {
            pay(&mut s, FUEL_HOST_CALL.saturating_add(bytes(len)))?;
            let t = &mut s.data_mut().table;
            let r = streams::HostOutputStream::splice(t, this, src, len);
            Ok((stream(t, r)?,))
        },
    )?;
    i.func_wrap(
        "[method]output-stream.blocking-splice",
        |mut s: StoreContextMut<'_, StoreData>,
         (this, src, len): (Resource<DynOutputStream>, Resource<DynInputStream>, u64)| {
            pay(&mut s, FUEL_HOST_CALL.saturating_add(bytes(len)))?;
            let t = &mut s.data_mut().table;
            let r = streams::HostOutputStream::blocking_splice(t, this, src, len);
            Ok((stream(t, r)?,))
        },
    )?;
    i.func_wrap(
        "[method]input-stream.read",
        |mut s: StoreContextMut<'_, StoreData>, (this, len): (Resource<DynInputStream>, u64)| {
            pay(&mut s, FUEL_HOST_CALL)?;
            let r = streams::HostInputStream::read(&mut s.data_mut().table, this, len);
            pay(&mut s, bytes(r.as_ref().map_or(0, |b| b.len() as u64)))?;
            let t = &mut s.data_mut().table;
            Ok((stream(t, r)?,))
        },
    )?;
    i.func_wrap(
        "[method]input-stream.blocking-read",
        |mut s: StoreContextMut<'_, StoreData>, (this, len): (Resource<DynInputStream>, u64)| {
            pay(&mut s, FUEL_HOST_CALL)?;
            let r = streams::HostInputStream::blocking_read(&mut s.data_mut().table, this, len);
            pay(&mut s, bytes(r.as_ref().map_or(0, |b| b.len() as u64)))?;
            let t = &mut s.data_mut().table;
            Ok((stream(t, r)?,))
        },
    )?;
    Ok(())
}

/// A stream call's answer as the guest sees it: the error in the WIT result, a trap as a trap.
fn stream<T>(
    table: &mut ResourceTable,
    result: StreamResult<T>,
) -> wasmtime::Result<Result<T, streams::StreamError>> {
    match result {
        Ok(v) => Ok(Ok(v)),
        Err(e) => Ok(Err(streams::Host::convert_stream_error(table, e)?)),
    }
}
