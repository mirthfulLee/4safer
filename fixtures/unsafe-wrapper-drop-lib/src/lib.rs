pub struct DroppableCounter {
    value: u32,
}

impl DroppableCounter {
    pub fn new(seed: u32) -> Self {
        Self { value: seed }
    }

    pub fn bump(&mut self) {
        unsafe {
            let ptr = &mut self.value as *mut u32;
            *ptr = (*ptr).wrapping_add(1);
        }
    }

    pub fn get(&self) -> u32 {
        self.value
    }
}

impl Drop for DroppableCounter {
    fn drop(&mut self) {
        self.value = self.value.wrapping_add(1);
    }
}
