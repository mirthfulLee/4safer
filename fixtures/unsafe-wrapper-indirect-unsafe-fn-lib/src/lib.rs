pub struct Counter {
    value: u32,
}

fn bump_core(value: &mut u32) {
    unsafe {
        let ptr = value as *mut u32;
        *ptr += 1;
    }
}

impl Counter {
    pub fn new(seed: u32) -> Self {
        Self { value: seed }
    }

    pub unsafe fn unsafe_bump(&mut self) {
        bump_core(&mut self.value);
    }

    pub fn safe_bump(&mut self) {
        bump_core(&mut self.value);
    }

    pub fn value(&self) -> u32 {
        self.value
    }
}
