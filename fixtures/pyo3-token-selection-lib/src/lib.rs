pub mod marker {
    use core::marker::PhantomData;

    #[derive(Clone, Copy)]
    pub struct Python<'py>(PhantomData<&'py ()>);

    impl Python<'_> {
        pub fn initialize() {}

        pub fn attach<F, R>(f: F) -> R
        where
            F: for<'py> FnOnce(Python<'py>) -> R,
        {
            f(Python(PhantomData))
        }
    }

    impl<'py> Python<'py> {
        pub fn check_flag(self, value: bool) -> bool {
            unsafe { read_flag(value) }
        }
    }

    unsafe fn read_flag(value: bool) -> bool {
        let ptr = &value as *const bool;
        unsafe { *ptr }
    }
}

pub use marker::Python;

pub struct Opaque {
    value: u32,
}

pub fn blocked_first(input: Opaque) -> u32 {
    unsafe { read_opaque(input) }
}

pub fn blocked_second(input: Opaque) -> u32 {
    unsafe { read_opaque(input) }
}

unsafe fn read_opaque(input: Opaque) -> u32 {
    input.value
}
