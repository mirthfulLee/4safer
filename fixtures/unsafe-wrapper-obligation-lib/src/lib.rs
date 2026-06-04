pub struct SinkState {
    limit: u32,
}

pub struct InputState {
    trusted: u32,
    unrelated: u32,
}

pub struct Wrapper {
    sink: SinkState,
    input: InputState,
}

impl Wrapper {
    pub fn new(seed: u32) -> Self {
        Self {
            sink: SinkState { limit: 0 },
            input: InputState {
                trusted: seed,
                unrelated: 0,
            },
        }
    }

    pub fn set_trusted(&mut self, value: u32) {
        self.input.trusted = value;
    }

    pub fn set_unrelated(&mut self, value: u32) {
        self.input.unrelated = value;
    }

    pub fn copy_trusted_into_limit(&mut self) {
        self.sink.limit = self.input.trusted;
    }

    pub fn copy_unrelated_into_limit(&mut self) {
        self.sink.limit = self.input.unrelated;
    }

    fn direct_unsafe_core(&mut self) {
        unsafe {
            let ptr = &mut self.sink.limit as *mut u32;
            *ptr += 1;
        }
    }

    pub fn unsafe_wrapper(&mut self) {
        self.direct_unsafe_core();
    }
}
