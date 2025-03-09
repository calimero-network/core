use core::any::{type_name, TypeId};
use core::marker::PhantomData;
use core::mem::transmute;
use core::ptr;
use std::rc::Rc;
use std::sync::Arc;

// https://github.com/sagebind/castaway/pull/14

/// Produces type IDs that are compatible with `TypeId::of::<T>`, but without
/// `T: 'static` bound.
fn non_static_type_id<T: ?Sized>() -> TypeId {
    trait NonStaticAny {
        fn get_type_id(&self) -> TypeId
        where
            Self: 'static;
    }

    impl<T: ?Sized> NonStaticAny for PhantomData<T> {
        fn get_type_id(&self) -> TypeId
        where
            Self: 'static,
        {
            TypeId::of::<T>()
        }
    }

    let phantom_data = PhantomData::<T>;
    NonStaticAny::get_type_id(unsafe {
        transmute::<&dyn NonStaticAny, &(dyn NonStaticAny + 'static)>(&phantom_data)
    })
}

pub trait Reflect {
    fn type_id(&self) -> TypeId {
        non_static_type_id::<Self>()
    }

    fn type_name(&self) -> &'static str {
        type_name::<Self>()
    }

    fn as_dyn_ref<'a>(&self) -> &(dyn Reflect + 'a)
    where
        Self: 'a;

    fn as_dyn_mut<'a>(&mut self) -> &mut (dyn Reflect + 'a)
    where
        Self: 'a;

    fn as_dyn_box<'a>(self: Box<Self>) -> Box<dyn Reflect + 'a>
    where
        Self: 'a;

    fn as_dyn_rc<'a>(self: Rc<Self>) -> Rc<dyn Reflect + 'a>
    where
        Self: 'a;

    fn as_dyn_arc<'a>(self: Arc<Self>) -> Arc<dyn Reflect + 'a>
    where
        Self: 'a;
}

impl<T> Reflect for T {
    fn as_dyn_ref<'a>(&self) -> &(dyn Reflect + 'a)
    where
        T: 'a,
    {
        self
    }

    fn as_dyn_mut<'a>(&mut self) -> &mut (dyn Reflect + 'a)
    where
        T: 'a,
    {
        self
    }

    fn as_dyn_box<'a>(self: Box<Self>) -> Box<dyn Reflect + 'a>
    where
        T: 'a,
    {
        self
    }

    fn as_dyn_rc<'a>(self: Rc<Self>) -> Rc<dyn Reflect + 'a>
    where
        T: 'a,
    {
        self
    }

    fn as_dyn_arc<'a>(self: Arc<Self>) -> Arc<dyn Reflect + 'a>
    where
        T: 'a,
    {
        self
    }
}

pub trait ReflectExt: Reflect {
    fn is<T: Reflect + ?Sized>(&self) -> bool {
        self.type_id() == non_static_type_id::<T>()
    }

    fn type_id() -> TypeId {
        non_static_type_id::<Self>()
    }

    fn downcast_ref<T: Reflect>(&self) -> Option<&T> {
        if self.is::<T>() {
            return Some(unsafe { &*ptr::from_ref(self).cast() });
        }

        None
    }

    fn downcast_mut<T: Reflect>(&mut self) -> Option<&mut T> {
        if self.is::<T>() {
            return Some(unsafe { &mut *ptr::from_mut(self).cast() });
        }

        None
    }

    fn downcast_box<T: Reflect>(self: Box<Self>) -> Result<Box<T>, Box<Self>> {
        if (&*self).is::<T>() {
            return Ok(unsafe { Box::from_raw(Box::into_raw(self).cast()) });
        }

        Err(self)
    }

    fn downcast_rc<T: Reflect>(self: Rc<Self>) -> Result<Rc<T>, Rc<Self>> {
        if (&*self).is::<T>() {
            return Ok(unsafe { Rc::from_raw(Rc::into_raw(self).cast()) });
        }

        Err(self)
    }

    fn downcast_arc<T: Reflect>(self: Arc<Self>) -> Result<Arc<T>, Arc<Self>> {
        if (&*self).is::<T>() {
            return Ok(unsafe { Arc::from_raw(Arc::into_raw(self).cast()) });
        }

        Err(self)
    }
}

impl<T: Reflect + ?Sized> ReflectExt for T {}
