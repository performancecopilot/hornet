extern crate hornet;

use hornet::mmv;
use std::env;
use std::path::Path;

fn main() {
    let path_arg = env::args().nth(1)
        .expect("Specify path to mmv file");
    let mmv_path = Path::new(&path_arg);

    print!("{}", mmv::dump(&mmv_path).unwrap());
}
