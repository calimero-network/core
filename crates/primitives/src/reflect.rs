use core::any::{type_name, TypeId};
use core::marker::PhantomData;
use core::mem::{transmute, ManuallyDrop};
use core::ptr;
use std::rc::Rc;

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

pub trait Reflect: DynReflect {
    fn type_id(&self) -> TypeId {
        non_static_type_id::<Self>()
    }

    fn type_name(&self) -> &'static str {
        type_name::<Self>()
    }
}

impl dyn Reflect + '_ {
    pub fn is<T: Reflect>(&self) -> bool {
        self.type_id() == non_static_type_id::<T>()
    }

    pub fn downcast_ref<T: Reflect>(&self) -> Option<&T> {
        if self.is::<T>() {
            #[allow(trivial_casts)]
            return Some(unsafe { &*ptr::from_ref::<dyn Reflect>(self).cast::<T>() });
        }

        None
    }

    pub fn downcast_box<T: Reflect>(self: Box<Self>) -> Result<Box<T>, Box<Self>> {
        if self.is::<T>() {
            return Ok(unsafe { Box::from_raw(Box::into_raw(self).cast::<T>()) });
        }
        Err(self)
    }

    pub fn downcast_rc<T: Reflect>(self: Rc<Self>) -> Result<Rc<T>, Rc<Self>> {
        if self.is::<T>() {
            return Ok(unsafe { Rc::from_raw(Rc::into_raw(self) as *mut T) });
        }

        Err(self)
    }
}

impl<T> Reflect for T {}

pub trait DynReflect {
    fn as_dyn<'a>(&self) -> &(dyn Reflect + 'a)
    where
        Self: 'a;

    fn as_dyn_box<'a>(self: Box<Self>) -> Box<dyn Reflect + 'a>
    where
        Self: 'a;

    fn as_dyn_rc<'a>(self: Rc<Self>) -> Rc<dyn Reflect + 'a>
    where
        Self: 'a;
}

impl<T> DynReflect for T {
    fn as_dyn<'a>(&self) -> &(dyn Reflect + 'a)
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
}

pub trait ReflectExt<'a>: Reflect
where
    Self: 'a,
{
    fn with_boxed<
        T: Reflect,
        F: FnOnce(Box<dyn Reflect + 'a>) -> Result<Box<T>, Box<dyn Reflect + 'a>>,
    >(
        self: Box<Self>,
        f: F,
    ) -> Result<Box<T>, Box<Self>> {
        let ptr = Box::into_raw(self);
        match f(unsafe { Box::from_raw(ptr) }.as_dyn_box()) {
            Ok(value) => Ok(value),
            Err(value) => {
                let _ = ManuallyDrop::new(value);
                Err(unsafe { Box::from_raw(ptr) })
            }
        }
    }

    fn with_rc<
        T: Reflect,
        F: FnOnce(Rc<dyn Reflect + 'a>) -> Result<Rc<T>, Rc<dyn Reflect + 'a>>,
    >(
        self: Rc<Self>,
        f: F,
    ) -> Result<Rc<T>, Rc<Self>> {
        let ptr = Rc::into_raw(self);
        match f(unsafe { Rc::from_raw(ptr) }.as_dyn_rc()) {
            Ok(value) => Ok(value),
            Err(value) => {
                let _ = ManuallyDrop::new(value);
                Err(unsafe { Rc::from_raw(ptr) })
            }
        }
    }
}

impl<'a, T: Reflect + ?Sized + 'a> ReflectExt<'a> for T {}
