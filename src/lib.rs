extern crate byteorder;
extern crate memmap;
extern crate regex;
extern crate time;
#[macro_use] extern crate bitflags;
#[macro_use] extern crate lazy_static;
#[cfg(test)] extern crate rand;
#[cfg(unix)] extern crate nix;
#[cfg(windows)] extern crate kernel32;

const CLUSTER_ID_BIT_LEN: usize = 12;
const NUMERIC_VALUE_SIZE: usize = 8;
const HDR_LEN: u64 = 40;
const TOC_BLOCK_LEN: u64 = 16;
const METRIC_BLOCK_LEN: u64 = 104;
const VALUE_BLOCK_LEN: u64 = 32;
const STRING_BLOCK_LEN: u64 = 256;
const METRIC_NAME_MAX_LEN: u64 = 64;
const MAX_STRINGS_PER_METRIC: u64 = 3;
const TOC_BLOCK_COUNT: u64 = 3;

type Endian = byteorder::LittleEndian;

pub mod client;
