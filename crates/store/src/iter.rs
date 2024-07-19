use std::fmt;
use std::marker::PhantomData;

use calimero_primitives::reflect::Reflect;

use crate::entry::DataType;
use crate::key::{FromKeyParts, Key as KeyCore}; // rename key here to KeyBuf
use crate::slice::Slice;

#[derive(Debug)]
pub struct Iter<'a, K = Unstructured, V = Unstructured> {
    inner: Box<dyn DBIter + 'a>,
    _priv: PhantomData<(K, V)>,
}

pub trait DBIter {
    fn next(&mut self) -> eyre::Result<Option<Key>>;
    fn read(&self) -> Option<Value>;
}

impl<'a> fmt::Debug for dyn DBIter + 'a {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.type_name())
    }
}

impl<'a, K, V> Iter<'a, K, V> {
    pub fn new<T: DBIter + 'a>(inner: T) -> Self {
        Self {
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

impl<'a, V> Iter<'a, Unstructured, V> {
    pub fn structured_key<K: FromKeyParts>(self) -> Iter<'a, Structured<K>, V> {
        Iter {
            inner: self.inner,
            _priv: PhantomData,
        }
    }
}

impl<'a, K> Iter<'a, K, Unstructured> {
    pub fn structured_value<'b, V: DataType<'b>>(self) -> Iter<'a, K, Structured<V>> {
        Iter {
            inner: self.inner,
            _priv: PhantomData,
        }
    }
}

type Key<'a> = Slice<'a>;
type Value<'a> = Slice<'a>;

impl<'a, K> DBIter for Iter<'a, K, Unstructured> {
    fn next(&mut self) -> eyre::Result<Option<Key>> {
        self.inner.next()
    }

    fn read(&self) -> Option<Value> {
        self.inner.read()
    }
}

pub struct IterKeys<'a, 'b, K, V> {
    iter: &'a mut Iter<'b, K, V>,
}

impl<'a, 'b, K: TryIntoKey<'b>, V> Iterator for IterKeys<'a, 'b, K, V> {
    type Item = K::Key;

    fn next(&mut self) -> Option<Self::Item> {
        let key = self.iter.inner.next().ok()??;

        // safety: key only needs to live as long as the iterator, not it's reference
        let key = unsafe { std::mem::transmute(key) };

        Some(K::try_into_key(key).ok()?)
    }
}

pub struct IterEntries<'a, 'b, K, V> {
    iter: &'a mut Iter<'b, K, V>,
}

impl<'a, 'b, K: TryIntoKey<'b>, V: TryIntoValue<'b>> Iterator for IterEntries<'a, 'b, K, V> {
    type Item = (K::Key, V::Value);

    fn next(&mut self) -> Option<Self::Item> {
        let key = {
            let key = self.iter.inner.next().ok()??;

            // safety: key only needs to live as long as the iterator, not it's reference
            unsafe { std::mem::transmute(key) }
        };

        let value = {
            let value = self.iter.inner.read()?;

            // safety: value only needs to live as long as the iterator, not it's reference
            unsafe { std::mem::transmute(value) }
        };

        Some((K::try_into_key(key).ok()?, V::try_into_value(value).ok()?))
    }
}

pub struct Structured<K> {
    _priv: PhantomData<K>,
}

pub struct Unstructured {
    _priv: (),
}

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

pub enum Error<E> {
    SizeMismatch,
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

impl<'a, V: DataType<'a>> TryIntoValue<'a> for Structured<V> {
    type Value = V;
    type Error = Error<V::Error>;

    fn try_into_value(value: Value<'a>) -> Result<Self::Value, Self::Error> {
        V::from_slice(value).map_err(Error::Structured)
    }
}

impl private::Sealed for Unstructured {}
impl<'a> TryIntoKey<'a> for Unstructured {
    type Key = Key<'a>;
    type Error = ();

    fn try_into_key(key: Key<'a>) -> Result<Self::Key, Self::Error> {
        Ok(key)
    }
}

impl<'a> TryIntoValue<'a> for Unstructured {
    type Value = Value<'a>;
    type Error = ();

    fn try_into_value(value: Value<'a>) -> Result<Self::Value, Self::Error> {
        Ok(value)
    }
}

pub struct IterPair<A, B>(pub A, pub B);

impl<A, B> DBIter for IterPair<A, B>
where
    A: DBIter,
    B: DBIter,
{
    fn next(&mut self) -> eyre::Result<Option<Key>> {
        let Some(key) = self.0.next()? else {
            return self.1.next();
        };

        Ok(Some(key))
    }

    fn read(&self) -> Option<Value> {
        self.0.read().or_else(|| self.1.read())
    }
}
