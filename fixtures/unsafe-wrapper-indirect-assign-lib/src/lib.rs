pub struct Counter {
    value: u32,
    label: String,
}

pub fn bump_scalar(value: &mut u32) {
    unsafe {
        let ptr = value as *mut u32;
        *ptr += 1;
    }
}

impl Counter {
    pub fn new(seed: u32) -> Self {
        Self {
            value: seed,
            label: String::new(),
        }
    }

    pub fn rename(&mut self, label: String) {
        self.label = label;
    }

    pub fn write_back_via_temp(&mut self) {
        let mut temp = 0u32;
        bump_scalar(&mut temp);
        self.value = temp;
    }

    pub fn write_label_via_temp(&mut self) {
        let mut temp = 0u32;
        bump_scalar(&mut temp);
        self.label = temp.to_string();
    }

    pub fn drop_unsafe_temp(&mut self) {
        let mut temp = 0u32;
        bump_scalar(&mut temp);
    }

    pub fn value(&self) -> u32 {
        self.value
    }
}
