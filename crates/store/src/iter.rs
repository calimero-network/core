use std::fmt;

use calimero_primitives::reflect::Reflect;

use crate::slice::Slice;

#[derive(Debug)]
pub struct Iter<'a> {
    inner: Box<dyn DBIter + 'a>,
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

impl<'a> Iter<'a> {
    pub fn new<T: DBIter + 'a>(inner: T) -> Self {
        Self {
            inner: Box::new(inner),
        }
    }

    pub fn keys(&mut self) -> IterKeys<'_, 'a> {
        IterKeys { iter: self }
    }

    pub fn entries(&mut self) -> IterEntries<'_, 'a> {
        IterEntries { iter: self }
    }
}

impl<'a> DBIter for Iter<'a> {
    fn next(&mut self) -> eyre::Result<Option<Key>> {
        self.inner.next()
    }

    fn read(&self) -> Option<Value> {
        self.inner.read()
    }
}

pub struct IterKeys<'a, 'b> {
    iter: &'a mut Iter<'b>,
}

type Key<'a> = Slice<'a>;

impl<'a, 'b> Iterator for IterKeys<'a, 'b> {
    type Item = Key<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        let key = self.iter.inner.next().ok()??;

        // safety: key only needs to live as long as the iterator, not it's reference
        let key = unsafe { std::mem::transmute(key) };

        Some(key)
    }
}

pub struct IterEntries<'a, 'b> {
    iter: &'a mut Iter<'b>,
}

type Value<'a> = Slice<'a>;

pub struct Entry<'a> {
    pub key: Key<'a>,
    pub value: Value<'a>,
}

impl<'a, 'b> Iterator for IterEntries<'a, 'b> {
    type Item = Entry<'a>;

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

        Some(Entry { key, value })
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
