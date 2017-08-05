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
const ITEM_BIT_LEN: usize = 10;
const INDOM_BIT_LEN: usize = 22;

const HDR_LEN: u64 = 40;
const TOC_BLOCK_LEN: u64 = 16;
const INDOM_BLOCK_LEN: u64 = 32;
const VALUE_BLOCK_LEN: u64 = 32;
const NUMERIC_VALUE_SIZE: usize = 8;
const STRING_BLOCK_LEN: u64 = 256;

const INSTANCE_BLOCK_LEN_MMV1: u64 = 80;
const METRIC_BLOCK_LEN_MMV1: u64 = 104;
const METRIC_NAME_MAX_LEN: u64 = 64;

const INSTANCE_BLOCK_LEN_MMV2: u64 = 24;
const METRIC_BLOCK_LEN_MMV2: u64 = 48;

type Endian = byteorder::LittleEndian;

#[macro_use]
mod private;

pub mod client;
pub mod mmv;
