pub struct Inner {
    limit: u32,
}

pub struct Outer {
    inner: Inner,
}

impl Inner {
    pub fn new(seed: u32) -> Self {
        Self { limit: seed }
    }
}

impl Outer {
    pub fn new(inner: Inner) -> Self {
        Self { inner }
    }
}

fn direct_unsafe_core(inner: &Inner) -> u32 {
    unsafe {
        let ptr = &inner.limit as *const u32;
        *ptr
    }
}

pub fn inspect(outer: &Outer) -> u32 {
    direct_unsafe_core(&outer.inner)
}
