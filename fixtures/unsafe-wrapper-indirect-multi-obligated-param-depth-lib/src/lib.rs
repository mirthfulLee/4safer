pub struct Handle {
    value: u32,
    scratch: u32,
    tag: &'static str,
}

pub struct Token {
    value: u32,
    tag: &'static str,
}

pub struct StartA(u32);
pub struct MidA(u32);
pub struct StartB(u32);
pub struct MidB(u32);
pub struct StartC(u32);
pub struct MidC(u32);

impl Handle {
    pub fn blank(seed: u32) -> Self {
        Self {
            value: 0,
            scratch: seed,
            tag: "handle",
        }
    }

    pub fn set_value(&mut self, value: u32) {
        self.value = value + self.scratch;
    }

    pub fn start_a(seed: u32) -> StartA {
        StartA(seed + 1)
    }

    pub fn start_b(seed: u32) -> StartB {
        StartB(seed + 2)
    }

    pub fn start_c(seed: u32) -> StartC {
        StartC(seed + 3)
    }

    pub fn mid_a(start: StartA) -> MidA {
        MidA(start.0 + 10)
    }

    pub fn mid_b(start: StartB) -> MidB {
        MidB(start.0 + 20)
    }

    pub fn mid_c(start: StartC) -> MidC {
        MidC(start.0 + 30)
    }

    pub fn finish_a(mid: MidA) -> Self {
        Self {
            value: mid.0,
            scratch: 0,
            tag: "handle",
        }
    }

    pub fn finish_b(mid: MidB) -> Self {
        Self {
            value: mid.0,
            scratch: 0,
            tag: "handle",
        }
    }

    pub fn finish_c(mid: MidC) -> Self {
        Self {
            value: mid.0,
            scratch: 0,
            tag: "handle",
        }
    }
}

impl Token {
    pub fn new(seed: u32) -> Self {
        Self {
            value: seed,
            tag: "token",
        }
    }
}

unsafe fn core(left: Handle, right: Handle, token: Token) -> u32 {
    unsafe {
        let lhs = &left.value as *const u32;
        let rhs = &right.value as *const u32;
        *lhs + *rhs + token.value + left.tag.len() as u32 + right.tag.len() as u32 + token.tag.len() as u32
    }
}

pub fn inspect(left: Handle, right: Handle, token: Token) -> u32 {
    unsafe { core(left, right, token) }
}
