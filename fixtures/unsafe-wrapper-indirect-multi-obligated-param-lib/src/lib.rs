pub struct State {
    left: u32,
    right: u32,
}

impl State {
    pub fn new(seed: u32) -> Self {
        Self {
            left: seed,
            right: seed,
        }
    }

    pub fn set_left(&mut self, value: u32) {
        self.left = value;
    }

    pub fn set_right(&mut self, value: u32) {
        self.right = value;
    }
}

unsafe fn core(a: &State, b: &State) -> u32 {
    let lhs = &a.left as *const u32;
    let rhs = &b.right as *const u32;
    unsafe { *lhs + *rhs }
}

pub fn inspect(a: &State, b: &State) -> u32 {
    unsafe { core(a, b) }
}
