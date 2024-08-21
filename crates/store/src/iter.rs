use std::fmt;
use std::marker::PhantomData;

use calimero_primitives::reflect::Reflect;
use thiserror::Error;

use crate::entry::Codec;
use crate::key::{FromKeyParts, Key as KeyCore};
use crate::slice::Slice;

pub struct Iter<'a, K = Unstructured, V = Unstructured> {
    done: bool,
    inner: Box<dyn DBIter + 'a>,
    _priv: PhantomData<(K, V)>,
}

impl<K, V> fmt::Debug for Iter<'_, K, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Iter").field(&self.inner).finish()
    }
}

pub trait DBIter {
    // todo! indicate somehow that Key<'a> doesn't contain mutable references to &'a mut self
    fn seek(&mut self, key: Key<'_>) -> eyre::Result<Option<Key<'_>>>;
    fn next(&mut self) -> eyre::Result<Option<Key<'_>>>;
    fn read(&self) -> eyre::Result<Value<'_>>;
}

impl fmt::Debug for dyn DBIter + '_ {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.type_name())
    }
}

impl<'a, K, V> Iter<'a, K, V> {
    pub fn new<T: DBIter + 'a>(inner: T) -> Self {
        Self {
            done: false,
            inner: Box::new(inner),
            _priv: PhantomData,
        }
    }

    pub fn keys(&mut self) -> IterKeys<'_, 'a, K, V> {
        IterKeys { iter: self }
    }

    pub fn entries(&mut self) -> IterEntries<'_, 'a, K, V> {
        IterEntries { iter: self }
    }
}

impl<V> Iter<'_, Unstructured, V> {
    pub fn seek(&mut self, key: Key<'_>) -> eyre::Result<Option<Key<'_>>> {
        self.inner.seek(key)
    }

    pub fn next(&mut self) -> eyre::Result<Option<Key<'_>>> {
        self.inner.next()
    }
}

impl<K: FromKeyParts, V> Iter<'_, Structured<K>, V>
where
    eyre::Report: From<Error<K::Error>>,
{
    pub fn seek(&mut self, key: K) -> eyre::Result<Option<K>> {
        let Some(key) = self.inner.seek(key.as_key().as_slice())? else {
            return Ok(None);
        };

        Ok(Some(Structured::<K>::try_into_key(key)?))
    }

    pub fn next(&mut self) -> eyre::Result<Option<K>> {
        let Some(key) = self.inner.next()? else {
            return Ok(None);
        };

        Ok(Some(Structured::<K>::try_into_key(key)?))
    }
}

impl<K> Iter<'_, K, Unstructured> {
    pub fn read(&self) -> eyre::Result<Value<'_>> {
        self.inner.read()
    }
}

impl<'a, K, V, C> Iter<'a, K, Structured<(V, C)>>
where
    C: Codec<'a, V>,
    eyre::Report: From<Error<C::Error>>,
{
    pub fn read(&'a self) -> eyre::Result<V> {
        Structured::<(V, C)>::try_into_value(self.inner.read()?).map_err(Into::into)
    }
}

impl<'a, V> Iter<'a, Unstructured, V> {
    pub fn structured_key<K: FromKeyParts>(self) -> Iter<'a, Structured<K>, V> {
        Iter {
            done: self.done,
            inner: self.inner,
            _priv: PhantomData,
        }
    }
}

impl<'a, K> Iter<'a, K, Unstructured> {
    pub fn structured_value<'b, V, C: Codec<'b, V>>(self) -> Iter<'a, K, Structured<(V, C)>> {
        Iter {
            done: self.done,
            inner: self.inner,
            _priv: PhantomData,
        }
    }
}

type Key<'a> = Slice<'a>;
type Value<'a> = Slice<'a>;

impl<K> DBIter for Iter<'_, K, Unstructured> {
    fn seek(&mut self, key: Key<'_>) -> eyre::Result<Option<Key<'_>>> {
        self.inner.seek(key)
    }

    fn next(&mut self) -> eyre::Result<Option<Key<'_>>> {
        if !self.done {
            if let Some(key) = self.inner.next()? {
                return Ok(Some(key));
            }
        }

        self.done = true;
        Ok(None)
    }

    fn read(&self) -> eyre::Result<Value<'_>> {
        self.inner.read()
    }
}

pub struct IterKeys<'a, 'b, K, V> {
    iter: &'a mut Iter<'b, K, V>,
}

impl<'b, K: TryIntoKey<'b>, V> Iterator for IterKeys<'_, 'b, K, V>
where
    eyre::Report: From<K::Error>,
{
    type Item = eyre::Result<K::Key>;

    fn next(&mut self) -> Option<Self::Item> {
        if !self.iter.done {
            match self.iter.inner.next() {
                Ok(Some(key)) => {
                    // safety: key only needs to live as long as the iterator, not it's reference
                    let key = unsafe { std::mem::transmute(key) };
                    return Some(K::try_into_key(key).map_err(Into::into));
                }
                Err(e) => return Some(Err(e)),
                Ok(None) => self.iter.done = true,
            }
        }
        None
    }
}

pub struct IterEntries<'a, 'b, K, V> {
    iter: &'a mut Iter<'b, K, V>,
}

impl<'b, K: TryIntoKey<'b>, V: TryIntoValue<'b>> Iterator for IterEntries<'_, 'b, K, V>
where
    eyre::Report: From<K::Error> + From<V::Error>,
{
    type Item = (eyre::Result<K::Key>, eyre::Result<V::Value>);

    fn next(&mut self) -> Option<Self::Item> {
        let key = 'key: {
            let key = 'found: {
                if !self.iter.done {
                    match self.iter.inner.next() {
                        Ok(Some(key)) => break 'found key,
                        Err(e) => break 'key Err(e),
                        _ => {}
                    }

                    self.iter.done = true;
                }

                return None;
            };

            // safety: key only needs to live as long as the iterator, not it's reference
            let key = unsafe { std::mem::transmute(key) };

            K::try_into_key(key).map_err(Into::into)
        };

        let value = 'value: {
            let value = match self.iter.inner.read() {
                Ok(value) => value,
                Err(value) => break 'value Err(value),
            };

            // safety: value only needs to live as long as the iterator, not it's reference
            let value = unsafe { std::mem::transmute(value) };

            V::try_into_value(value).map_err(Into::into)
        };

        Some((key, value))
    }
}

pub struct Structured<K> {
    _priv: PhantomData<K>,
}

#[derive(Clone, Copy)]
pub enum Unstructured {}

mod private {
    pub trait Sealed {}
}

pub trait TryIntoKey<'a>: private::Sealed {
    type Key;
    type Error;

    fn try_into_key(key: Key<'a>) -> Result<Self::Key, Self::Error>;
}

pub trait TryIntoValue<'a>: private::Sealed {
    type Value;
    type Error;

    fn try_into_value(key: Value<'a>) -> Result<Self::Value, Self::Error>;
}

#[derive(Debug, Error)]
pub enum Error<E> {
    #[error("size mismatch")]
    SizeMismatch,
    #[error(transparent)]
    Structured(E),
}

impl<K> private::Sealed for Structured<K> {}
impl<'a, K: FromKeyParts> TryIntoKey<'a> for Structured<K> {
    type Key = K;
    type Error = Error<K::Error>;

    fn try_into_key(key: Key<'a>) -> Result<Self::Key, Self::Error> {
        let key = KeyCore::try_from_slice(key).ok_or(Error::SizeMismatch)?;

        K::try_from_parts(key).map_err(Error::Structured)
    }
}

impl<'a, V, C: Codec<'a, V>> TryIntoValue<'a> for Structured<(V, C)> {
    type Value = V;
    type Error = Error<C::Error>;

    fn try_into_value(value: Value<'a>) -> Result<Self::Value, Self::Error> {
        C::decode(value).map_err(Error::Structured)
    }
}

impl private::Sealed for Unstructured {}
impl<'a> TryIntoKey<'a> for Unstructured {
    type Key = Key<'a>;
    type Error = std::convert::Infallible;

    fn try_into_key(key: Key<'a>) -> Result<Self::Key, Self::Error> {
        Ok(key)
    }
}

impl<'a> TryIntoValue<'a> for Unstructured {
    type Value = Value<'a>;
    type Error = std::convert::Infallible;

    fn try_into_value(value: Value<'a>) -> Result<Self::Value, Self::Error> {
        Ok(value)
    }
}

enum FusedIter<I> {
    Active(I),
    Interregnum,
    Expended(I),
}

impl<I: DBIter> FusedIter<I> {
    fn seek(&mut self, key: Key<'_>) -> eyre::Result<Option<Key<'_>>> {
        if let FusedIter::Active(iter) = self {
            return iter.seek(key);
        }

        Ok(None)
    }

    fn next(&mut self) -> eyre::Result<Option<Key<'_>>> {
        let this = unsafe { &mut *(self as *mut Self) };

        if let FusedIter::Active(iter) = this {
            if let Some(key) = iter.next()? {
                return Ok(Some(key));
            }

            match std::mem::replace(self, FusedIter::Interregnum) {
                FusedIter::Active(iter) => *self = FusedIter::Expended(iter),
                _ => unsafe { std::hint::unreachable_unchecked() },
            }
        }

        Ok(None)
    }

    fn read(&self) -> eyre::Result<Option<Value<'_>>> {
        if let FusedIter::Active(iter) = self {
            return iter.read().map(Some);
        }

        Ok(None)
    }
}

pub struct IterPair<A, B>(FusedIter<A>, B);

impl<A, B> IterPair<A, B> {
    pub fn new(iter: A, other: B) -> Self {
        Self(FusedIter::Active(iter), other)
    }
}

impl<A, B> DBIter for IterPair<A, B>
where
    A: DBIter,
    B: DBIter,
{
    fn seek(&mut self, key: Key<'_>) -> eyre::Result<Option<Key<'_>>> {
        if let Some(key) = self.0.seek(key.as_ref().into())? {
            return Ok(Some(key));
        }

        self.1.seek(key)
    }

    fn next(&mut self) -> eyre::Result<Option<Key<'_>>> {
        if let Some(key) = self.0.next()? {
            return Ok(Some(key));
        }

        self.1.next()
    }

    fn read(&self) -> eyre::Result<Value<'_>> {
        if let Some(value) = self.0.read()? {
            return Ok(value);
        }

        self.1.read()
    }
}
