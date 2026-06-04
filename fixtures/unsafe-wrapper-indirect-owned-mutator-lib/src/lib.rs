#[derive(Clone)]
pub struct Data {
    value: u32,
}

pub struct State {
    data: Data,
}

pub fn owned_mutator(mut data: Data) {
    data.value = 9;
}

impl State {
    pub fn new(seed: u32) -> Self {
        Self {
            data: Data { value: seed },
        }
    }

    pub fn set_value(&mut self, value: u32) {
        self.data.value = value;
    }

    pub fn call_owned_mutator(&mut self) {
        owned_mutator(self.data.clone());
    }

    pub fn bump(&mut self) {
        unsafe {
            let ptr = &mut self.data.value as *mut u32;
            *ptr += 1;
        }
    }
}
