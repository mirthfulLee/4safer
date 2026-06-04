pub fn push_one(values: &mut Vec<u8>) {
    values.push(1);
}

unsafe fn direct_unsafe_core(values: &Vec<u8>) -> usize {
    values.len()
}

pub fn inspect(values: &Vec<u8>) -> usize {
    unsafe { direct_unsafe_core(values) }
}
