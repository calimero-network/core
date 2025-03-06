trait IsSized<const N: usize>: Sized {
    const MATCHES_EXPECTED_SIZE: () = [()][size_of::<Self>() - N];
}

impl<T, const N: usize> IsSized<N> for T {}

struct Uninhabited<T: ?Sized> {
    ptr: *const T,
}

impl<T: ?Sized> fmt::Debug for Uninhabited<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Uninhabited")
            .field("ptr", &self.ptr)
            .finish()
    }
}

impl<T: ?Sized> Uninhabited<T> {
    pub unsafe fn coerce<U: ?Sized>(self, func: fn(*const T) -> *const U) -> Uninhabited<U> {
        Uninhabited {
            ptr: std::hint::black_box(func(self.ptr)),
        }
    }
}

fn vtable_ptr<T, U: ?Sized>(func: fn(Uninhabited<T>) -> Uninhabited<U>) -> *const ()
where
    Uninhabited<U>: IsSized<16>,
{
    <Uninhabited<U>>::MATCHES_EXPECTED_SIZE;
    let tmp = std::hint::black_box(Uninhabited {
        ptr: std::hint::black_box(ptr::dangling()),
    });
    let tmp = std::hint::black_box(func(tmp));
    let tmp = std::hint::black_box(ptr::from_ref(&tmp.ptr));
    let tmp = std::hint::black_box(tmp as *const AbstractDynInner<U>);
    let tmp = std::hint::black_box(unsafe { &*tmp });
    std::hint::black_box(tmp.meta as _)
}

#[test]
fn theis() {
    trait Trait {}
    struct Struct;
    // impl Trait for Struct {}

    let f = vtable_ptr::<Struct, dyn Trait>(|u| unsafe {
        u.coerce(|f| {
            dbg!(f);
            let e = f as *const dyn Trait;
            dbg!(ptr::metadata(e));
            e
        })
    });

    dbg!(f);

    // let dud = MaybeUninit::<Struct>::uninit().as_ptr() as *const dyn Trait;
    let dud = ptr::dangling::<Struct>() as *const dyn Trait;

    let d = ptr::metadata(dud);

    dbg!(d);

    let unh = Uninhabited { ptr: dud };

    dbg!(&unh);

    let e = unsafe { unh.coerce(|f| f) };

    dbg!(&e);

    dbg!(ptr::metadata(e.ptr));

    let d = ptr::metadata(&Struct as &dyn Trait as *const dyn Trait);

    dbg!(d);

    let d = ptr::metadata(&Struct as *const Struct as *const dyn Trait);

    dbg!(d);
}
