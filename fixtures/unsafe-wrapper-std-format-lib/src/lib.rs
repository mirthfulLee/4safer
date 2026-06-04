pub struct Label {
    value: u32,
}

impl Label {
    pub fn new(value: u32) -> Self {
        Self { value }
    }

    pub fn render(&self) -> String {
        format!("label:{}", self.value)
    }
}
