pub struct A {
    value: u32,
}

pub struct B {
    source: u32,
    other: u32,
}

pub struct Container {
    a: A,
    b: B,
}

impl Container {
    pub fn new(seed: u32) -> Self {
        Self {
            a: A { value: seed },
            b: B {
                source: seed,
                other: 0,
            },
        }
    }

    pub fn set_source(&mut self, value: u32) {
        self.b.source = value;
    }

    pub fn set_other(&mut self, value: u32) {
        self.b.other = value;
    }

    pub fn bump_core(&mut self) {
        unsafe {
            let ptr = &mut self.a.value as *mut u32;
            *ptr += 1;
        }
    }

    pub fn copy_from_source_then_bump(&mut self) {
        self.a.value = self.b.source;
        self.bump_core();
    }

    pub fn copy_from_other_then_bump(&mut self) {
        self.a.value = self.b.other;
        self.bump_core();
    }
}
