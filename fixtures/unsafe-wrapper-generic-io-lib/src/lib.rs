use std::io::{Read, Seek, SeekFrom};

fn direct_unsafe_core(value: u8) -> u8 {
    unsafe {
        let ptr = &value as *const u8;
        *ptr
    }
}

pub fn inspect_read_seek<R: Read + Seek>(reader: &mut R) -> u8 {
    let mut buf = [0u8; 1];
    let _ = reader.read(&mut buf);
    let _ = reader.seek(SeekFrom::Start(0));
    direct_unsafe_core(buf[0])
}
