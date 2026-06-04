pub struct State {
    limit: u32,
    source: u32,
}

impl State {
    pub fn new(seed: u32) -> Self {
        Self {
            limit: 0,
            source: seed,
        }
    }

    pub fn set_source(&mut self, value: u32) {
        self.source = value;
    }

    pub fn copy_source_into_limit(&mut self) {
        self.limit = self.source;
    }
}

fn direct_unsafe_core(state: &State) -> u32 {
    unsafe {
        let ptr = &state.limit as *const u32;
        *ptr
    }
}

pub fn inspect(state: &State) -> u32 {
    direct_unsafe_core(state)
}
