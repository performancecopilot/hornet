extern crate hornet;

use hornet::mmv;
use std::fs;
use std::fs::File;
use std::io::prelude::*;
use std::path::PathBuf;

#[test]
fn test_mmvfmt() {
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    let mut testdata_dir = crate_dir.clone();
    testdata_dir.push("tests/data");
    let tests = fs::read_dir(&testdata_dir).unwrap().count() / 2;

    let input_prefix = "mmvdump_ip";
    let input_suffix = ".mmv";
    let output_prefix = "mmvdump_op";
    let output_suffix = ".golden";

    for i in 1..tests+1 {
        let mut output_path = testdata_dir.clone();
        output_path.push(&format!("{}{}{}", output_prefix, i, output_suffix));
        let mut golden_output = Vec::new();
        File::open(output_path).unwrap()
            .read_to_end(&mut golden_output).unwrap();

        let mut input_path = testdata_dir.clone();
        input_path.push(&format!("{}{}{}", input_prefix, i, input_suffix));
        let mmv = mmv::dump(&input_path).unwrap();
        let mut mmvdumm_output = Vec::new();
        write!(&mut mmvdumm_output, "{}", mmv).unwrap();

        assert_eq!(mmvdumm_output, golden_output);
    }
}
