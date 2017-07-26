use byteorder::ReadBytesExt;
use std::collections::BTreeMap;
use std::ffi::CStr;
use std::fmt;
use std::fs::File;
use std::io;
use std::io::Cursor;
use std::io::prelude::*;
use std::path::Path;
use std::str;

const INDOM_TOC_CODE: u32 = 1;
const INSTANCE_TOC_CODE: u32 = 2;
const METRIC_TOC_CODE: u32 = 3;
const VALUES_TOC_CODE: u32 = 4;
const STRINGS_TOC_CODE: u32 = 5;

#[derive(Copy, Clone)]
/// MMV code for a metric type
pub enum MTCode {
    /// 32-bit signed integer
    I32 = 0,
    /// 32-bit unsigned integer
    U32,
    /// 64-bit signed integer
    I64,
    /// 64-bit unsigned integer
    U64,
    /// 32-bit float
    F32,
    /// 64-bit double
    F64,
    /// String
    String
}

impl MTCode {
    pub fn from_u32(x: u32) -> Option<Self> {
        match x {
            0 => Some(MTCode::I32),
            1 => Some(MTCode::U32),
            2 => Some(MTCode::I64),
            3 => Some(MTCode::U64),
            4 => Some(MTCode::F32),
            5 => Some(MTCode::F64),
            6 => Some(MTCode::String),
            _ => None
        }
    }
}

impl fmt::Display for MTCode {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            MTCode::I32 => write!(f, "Int32")?,
            MTCode::U32 => write!(f, "Uint32")?,
            MTCode::I64 => write!(f, "Int64")?,
            MTCode::U64 => write!(f, "Uint64")?,
            MTCode::F32 => write!(f, "Float32")?,
            MTCode::F64 => write!(f, "Double64")?,
            MTCode::String => write!(f, "String")?
        }
        write!(f, " (0x{:x})", *self as u32)
    }
}

use super::{
    Endian,
    METRIC_NAME_MAX_LEN,
    STRING_BLOCK_LEN,
    CLUSTER_ID_BIT_LEN,
    ITEM_BIT_LEN,
    INDOM_BIT_LEN
};

fn is_valid_indom(indom: u32) -> bool {
    indom != 0 && (indom >> INDOM_BIT_LEN) == 0
}

fn is_valid_item(item: u32) -> bool {
    item != 0 && (item >> ITEM_BIT_LEN) == 0
}

fn is_valid_cluster_id(cluster_id: u32) -> bool {
    (cluster_id >> CLUSTER_ID_BIT_LEN) == 0
}

fn is_valid_blk_offset(offset: u64) -> bool {
    offset != 0
}

/// Error encountered while reading and parsing an MMV
#[derive(Debug)]
pub enum MMVDumpError {
    /// Invalid bytes in MMV
    InvalidMMV(String),
    /// IO error while reading MMV
    Io(io::Error),
    /// UTF-8 error while parsing MMV strings
    Utf8(str::Utf8Error)
}

impl From<io::Error> for MMVDumpError {
    fn from(err: io::Error) -> MMVDumpError {
        MMVDumpError::Io(err)
    }
}

impl From<str::Utf8Error> for MMVDumpError {
    fn from(err: str::Utf8Error) -> MMVDumpError {
        MMVDumpError::Utf8(err)
    }
}

macro_rules! return_mmvdumperror (
    ($err:expr, $val:expr) => (
        let mut err_str = $err.to_owned();
        err_str.push_str(&format!(": {}", $val));
        return Err(MMVDumpError::InvalidMMV(err_str));
    )
);

/// Trait for structures that read and parse MMV bytes
pub trait MMVReader {
    /// Reads and parses MMV bytes from reader `r` and returns the
    /// relevant structure
    fn from_reader<R: ReadBytesExt>(r: &mut R) -> Result<Self, MMVDumpError>
        where Self: Sized;
}

/// MMV structure
///
/// The various data blocks are stored in BTreeMaps; the key for each
/// block is it's offset in the MMV
pub struct MMV {
    pub header: Header,
    pub metric_toc: TOC,
    pub value_toc: TOC,
    pub string_toc: Option<TOC>,
    pub indom_toc: Option<TOC>,
    pub instance_toc: Option<TOC>,
    pub metric_blks: BTreeMap<u64, MetricBlk>,
    pub value_blks: BTreeMap<u64, ValueBlk>,
    pub string_blks: BTreeMap<u64, StringBlk>,
    pub indom_blks: BTreeMap<u64, IndomBlk>,
    pub instance_blks: BTreeMap<u64, InstanceBlk>
}

/// MMV header structure
pub struct Header {
    pub magic: [u8; 4],
    pub version: u32,
    pub gen1: i64,
    pub gen2: i64,
    pub toc_count: u32,
    pub flags: u32,
    pub pid: i32,
    pub cluster_id: u32,
}

impl MMVReader for Header {
    fn from_reader<R: ReadBytesExt>(r: &mut R) -> Result<Self, MMVDumpError> {
        let mut magic = [0; 4];
        magic[0] = r.read_u8()?;
        magic[1] = r.read_u8()?;
        magic[2] = r.read_u8()?;
        magic[3] = r.read_u8()?;
        if magic != [b'M', b'M', b'V', 0] {
            return_mmvdumperror!("Invalid MMV", 0);
        }

        let version = r.read_u32::<Endian>()?;
        if version != 1 && version != 2 {
            return_mmvdumperror!("Invalid version number", version);
        }

        let gen1 = r.read_i64::<Endian>()?;
        let gen2 = r.read_i64::<Endian>()?;
        if gen1 != gen2 {
            return_mmvdumperror!("Generation timestamps don't match", 0);
        } 

        let toc_count = r.read_u32::<Endian>()?;
        if toc_count > 5 || toc_count < 2 {
            return_mmvdumperror!("Invalid TOC count", toc_count);
        }

        let flags = r.read_u32::<Endian>()?;
        let pid = r.read_i32::<Endian>()?;

        let cluster_id = r.read_u32::<Endian>()?;
        if !is_valid_cluster_id(cluster_id) {
            return_mmvdumperror!("Invalid cluster ID", cluster_id);
        }

        Ok(Header {
            magic: magic,
            version: version,
            gen1: gen1,
            gen2: gen2,
            toc_count: toc_count,
            flags: flags,
            pid: pid,
            cluster_id: cluster_id
        })
    }
}

/// MMV Table-of-Contents structure
pub struct TOC {
    pub _mmv_offset: u64,
    pub sec: u32,
    pub entries: u32,
    pub sec_offset: u64
}

impl MMVReader for TOC {
    fn from_reader<R: ReadBytesExt>(r: &mut R) -> Result<Self, MMVDumpError> {
        let sec = r.read_u32::<Endian>()?;
        if sec > 5 {
            return_mmvdumperror!("Invalid TOC type", sec);
        }

        let entries = r.read_u32::<Endian>()?;

        let sec_offset = r.read_u64::<Endian>()?;
        if !is_valid_blk_offset(sec_offset) {
            return_mmvdumperror!("Invalid section offset", sec_offset);
        }

        Ok(TOC {
            _mmv_offset: 0,
            sec: sec,
            entries: entries,
            sec_offset: sec_offset
        })
    }
}

/// Metric block structure
pub struct MetricBlk {
    pub name: String,
    pub item: Option<u32>,
    pub typ: u32,
    pub sem: u32,
    pub unit: u32,
    pub indom: Option<u32>,
    pub pad: u32,
    pub short_help_offset: Option<u64>,
    pub long_help_offset: Option<u64>
}

impl MMVReader for MetricBlk {
    fn from_reader<R: ReadBytesExt>(r: &mut R) -> Result<Self, MMVDumpError> {
        let mut name_bytes = [0; METRIC_NAME_MAX_LEN as usize];
        r.read_exact(&mut name_bytes)?;
        let cstr = unsafe {
            CStr::from_ptr(name_bytes.as_ptr() as *const i8)
        };
        let name = cstr.to_str()?.to_owned();

        let item = r.read_u32::<Endian>()?;
        let typ = r.read_u32::<Endian>()?;
        let sem = r.read_u32::<Endian>()?;
        let unit = r.read_u32::<Endian>()?;
        let indom = r.read_u32::<Endian>()?;

        let pad = r.read_u32::<Endian>()?;
        if pad != 0 {
            return_mmvdumperror!("Invalid pad bytes", pad);
        }

        let short_help_offset = r.read_u64::<Endian>()?;
        let long_help_offset = r.read_u64::<Endian>()?;
        
        Ok(MetricBlk {
            name: name,
            item: {
                if is_valid_item(item) { Some(item) }
                else { None }
            },
            typ: typ,
            sem: sem,
            unit: unit,
            indom: {
                if is_valid_indom(indom) { Some(indom) }
                else { None }
            },
            pad: pad,
            short_help_offset: {
                if is_valid_blk_offset(short_help_offset) { Some(short_help_offset) }
                else { None }
            },
            long_help_offset: {
                if is_valid_blk_offset(long_help_offset) { Some(long_help_offset) }
                else { None }
            }
        })
    }
}

/// Value block structure
pub struct ValueBlk {
    pub value: u64,
    pub string_offset: Option<u64>,
    pub metric_offset: Option<u64>,
    pub instance_offset: Option<u64>
}

impl MMVReader for ValueBlk {
    fn from_reader<R: ReadBytesExt>(r: &mut R) -> Result<Self, MMVDumpError> {
        let value = r.read_u64::<Endian>()?;
        let string_offset = r.read_u64::<Endian>()?;
        let metric_offset = r.read_u64::<Endian>()?;
        let instance_offset = r.read_u64::<Endian>()?;

        Ok(ValueBlk {
            value: value,
            string_offset: {
                if is_valid_blk_offset(string_offset) { Some(string_offset) }
                else { None }
            },
            metric_offset: {
                if is_valid_blk_offset(metric_offset) { Some(metric_offset) }
                else { None }
            },
            instance_offset: {
                if is_valid_blk_offset(instance_offset) { Some(instance_offset) }
                else { None }
            },
        })
    }
}

/// Indom block structure
pub struct IndomBlk {
    pub indom: Option<u32>,
    pub instances: u32,
    pub instances_offset: Option<u64>,
    pub short_help_offset: Option<u64>,
    pub long_help_offset: Option<u64>
}

impl MMVReader for IndomBlk {
    fn from_reader<R: ReadBytesExt>(r: &mut R) -> Result<Self, MMVDumpError> {
        let indom = r.read_u32::<Endian>()?;
        let instances = r.read_u32::<Endian>()?;
        let instances_offset = r.read_u64::<Endian>()?;
        let short_help_offset = r.read_u64::<Endian>()?;
        let long_help_offset = r.read_u64::<Endian>()?;

        Ok(IndomBlk {
            indom: {
                if is_valid_indom(indom) { Some(indom) }
                else { None }
            },
            instances: instances,
            instances_offset: {
                if is_valid_blk_offset(instances_offset) { Some(instances_offset) }
                else { None }
            },
            short_help_offset: {
                if is_valid_blk_offset(short_help_offset) { Some(short_help_offset) }
                else { None }
            },
            long_help_offset: {
                if is_valid_blk_offset(long_help_offset) { Some(long_help_offset) }
                else { None }
            }
        })
    }
}

/// Instance block structure
pub struct InstanceBlk {
    pub indom_offset: Option<u64>,
    pub pad: u32,
    pub internal_id: i32,
    pub external_id: String
}

impl MMVReader for InstanceBlk {
    fn from_reader<R: ReadBytesExt>(r: &mut R) -> Result<Self, MMVDumpError> {
        let indom_offset = r.read_u64::<Endian>()?;

        let pad = r.read_u32::<Endian>()?;
        if pad != 0 {
            return_mmvdumperror!("Invalid pad bytes", pad);
        }

        let internal_id = r.read_i32::<Endian>()?;

        let mut external_id_bytes = [0; METRIC_NAME_MAX_LEN as usize];
        r.read_exact(&mut external_id_bytes)?;
        let cstr = unsafe {
            CStr::from_ptr(external_id_bytes.as_ptr() as *const i8)
        };
        let external_id = cstr.to_str()?.to_owned();

        Ok(InstanceBlk {
            indom_offset: {
                if is_valid_blk_offset(indom_offset) { Some(indom_offset) }
                else { None }
            },
            pad: pad,
            internal_id: internal_id,
            external_id: external_id
        })
    }
}

/// String block structure
pub struct StringBlk {
    pub string: String
}

impl MMVReader for StringBlk {
    fn from_reader<R: ReadBytesExt>(r: &mut R) -> Result<Self, MMVDumpError> {
        let mut bytes = [0; STRING_BLOCK_LEN as usize];
        r.read_exact(&mut bytes)?;
        let cstr = unsafe {
            CStr::from_ptr(bytes.as_ptr() as *const i8)
        };
        let string = cstr.to_str()?.to_owned();

        Ok(StringBlk {
            string: string
        })
    }
}

macro_rules! blks_from_toc (
    ($toc:expr, $blk_typ:tt, $cursor:expr) => (
        if let Some(ref toc) = $toc {
            let mut blks = BTreeMap::new();

            $cursor.set_position(toc.sec_offset);
            for _ in 0..toc.entries as usize {
                let blk_offset = $cursor.position();
                blks.insert(blk_offset, $blk_typ::from_reader(&mut $cursor)?);
            }

            blks
        } else {
            BTreeMap::new()
        }
    )
);

/// Returns an `MMV` structure by reading and parsing the MMV
/// file stored at `mmv_path`
pub fn dump(mmv_path: &Path) -> Result<MMV, MMVDumpError> {
    let mut mmv_bytes = Vec::new();
    let mut file = File::open(mmv_path)?;
    file.read_to_end(&mut mmv_bytes)?;

    let mut cursor = Cursor::new(mmv_bytes);
    
    let hdr = Header::from_reader(&mut cursor)?;

    let mut indom_toc = None;
    let mut instance_toc = None;
    let mut metric_toc = None;
    let mut value_toc = None;
    let mut string_toc = None;

    for _ in 0..hdr.toc_count {
        let toc_position = cursor.position();
        let mut toc = TOC::from_reader(&mut cursor)?;
        toc._mmv_offset = toc_position;

        if toc.sec == INDOM_TOC_CODE { indom_toc = Some(toc); }
        else if toc.sec == INSTANCE_TOC_CODE { instance_toc = Some(toc); }
        else if toc.sec == METRIC_TOC_CODE { metric_toc = Some(toc); }
        else if toc.sec == VALUES_TOC_CODE { value_toc = Some(toc); }
        else if toc.sec == STRINGS_TOC_CODE { string_toc = Some(toc); }
    }

    if metric_toc.is_none() {
        return_mmvdumperror!("Metric TOC absent", 0);
    }
    if value_toc.is_none() {
        return_mmvdumperror!("String TOC absent", 0);
    }

    let indom_blks = blks_from_toc!(indom_toc, IndomBlk, cursor);
    let instance_blks = blks_from_toc!(instance_toc, InstanceBlk, cursor);
    let metric_blks = blks_from_toc!(metric_toc, MetricBlk, cursor);
    let value_blks = blks_from_toc!(value_toc, ValueBlk, cursor);
    let string_blks = blks_from_toc!(string_toc, StringBlk, cursor);

    Ok(
        MMV {
            header: hdr,
            metric_toc: metric_toc.unwrap(),
            value_toc: value_toc.unwrap(),
            string_toc: string_toc,
            indom_toc: indom_toc,
            instance_toc: instance_toc,
            indom_blks: indom_blks,
            instance_blks: instance_blks,
            metric_blks: metric_blks,
            value_blks: value_blks,
            string_blks: string_blks
        }
    )
}
