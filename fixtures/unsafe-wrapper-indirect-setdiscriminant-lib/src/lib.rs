pub enum Mode {
    Off,
    On,
}

pub struct State {
    mode: Mode,
}

impl State {
    pub fn new() -> Self {
        Self { mode: Mode::Off }
    }

    pub fn set_on(&mut self) {
        self.mode = Mode::On;
    }
}

fn direct_unsafe_core(state: &State) -> u8 {
    unsafe {
        let ptr = &state.mode as *const Mode as *const u8;
        *ptr
    }
}

pub fn inspect(state: &State) -> u8 {
    direct_unsafe_core(state)
}
