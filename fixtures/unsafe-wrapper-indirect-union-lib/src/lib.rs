pub union Payload {
    limit: u32,
    other: u32,
}

pub struct State {
    payload: Payload,
}

impl State {
    pub fn new(seed: u32) -> Self {
        Self {
            payload: Payload { limit: seed },
        }
    }

    pub fn set_limit(&mut self, value: u32) {
        self.payload = Payload { limit: value };
    }

    pub fn set_other(&mut self, value: u32) {
        self.payload = Payload { other: value };
    }
}

pub fn inspect(state: &State) -> u32 {
    unsafe { state.payload.limit }
}
