pub struct State {
    value: u32,
}

pub struct Token {
    delta: u32,
}

impl State {
    pub fn new(seed: u32) -> Self {
        Self { value: seed }
    }

    pub fn recover(seed: u32) -> Self {
        Self {
            value: seed.saturating_add(1),
        }
    }
}

pub fn make_data(seed: u8) -> Vec<u8> {
    vec![seed, seed.wrapping_add(1)]
}

pub fn wrap_token(state: &mut State, token: Token) -> u32 {
    unsafe {
        let ptr = &state.value as *const u32;
        *ptr + token.delta
    }
}

pub fn wrap(state: &mut State, data: Vec<u8>) -> u32 {
    unsafe {
        let ptr = &state.value as *const u32;
        *ptr + data.len() as u32
    }
}
