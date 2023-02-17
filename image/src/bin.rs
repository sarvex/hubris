use std::env;
use std::fs::File;
use std::io::Read;
use image::header;

pub fn main() {
    for path in env::args().skip(1) {
        let buf = readfile(&path);
        match header(&buf.into_boxed_slice()) {
            Some((offset, h)) => {
                println!("{}: 0x{:X?}: {:#X?}", path, offset, h);
            },
            None => {
                println!("{}: None", path);
            },
        }
    }
}

fn readfile(filename: &String) -> Vec<u8> {
    let mut f = File::open(&filename).expect("file open error");
    let meta = std::fs::metadata(&filename).expect("unable to stat");
    let mut buf = vec![0; meta.len() as usize];
    f.read(&mut buf).expect("read error");
    buf
}
