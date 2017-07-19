use byteorder::ReadBytesExt;
use std::fs::File;
use std::io;
use std::io::Cursor;
use std::io::prelude::*;
use std::path::Path;

const INDOM_TOC_CODE: u32 = 1;
const INSTANCE_TOC_CODE: u32 = 2;
const METRIC_TOC_CODE: u32 = 3;
const VALUES_TOC_CODE: u32 = 4;
const STRINGS_TOC_CODE: u32 = 5;

use super::{
    Endian,
    METRIC_NAME_MAX_LEN,
    STRING_BLOCK_LEN,
    CLUSTER_ID_BIT_LEN
};

pub enum MMVDumpError {
    InvalidMMV(String),
    Io(io::Error)
}

impl From<io::Error> for MMVDumpError {
    fn from(err: io::Error) -> MMVDumpError {
        MMVDumpError::Io(err)
    }
}

macro_rules! return_mmvdumperror (
    ($err:expr) => (
        return Err(MMVDumpError::InvalidMMV($err.to_owned()));
    )
);

pub struct MMV {
    header: Header,
    metric_toc: TOC,
    value_toc: TOC,
    string_toc: Option<TOC>,
    indom_toc: Option<TOC>,
    instance_toc: Option<TOC>,
    metric_blks: Vec<MetricBlk>,
    value_blks: Vec<ValueBlk>,
    string_blks: Vec<StringBlk>,
    indom_blks: Vec<IndomBlk>,
    instance_blks: Vec<InstanceBlk>
}

pub struct Header {
    _mmv_offset: u64,
    magic: [u8; 4],
    version: u32,
    gen1: i64,
    gen2: i64,
    toc_count: u32,
    flags: u32,
    pid: i32,
    cluster_id: u32,
}

impl Header {
    fn from_cursor(c: &mut Cursor<Vec<u8>>) -> Result<Self, MMVDumpError> {
        let _mmv_offset = c.position();

        let mut magic = [0; 4];
        magic[0] = c.read_u8()?;
        magic[1] = c.read_u8()?;
        magic[2] = c.read_u8()?;
        magic[3] = c.read_u8()?;
        if magic != [b'M', b'M', b'V', 0] {
            return_mmvdumperror!("Invalid MMV");
        }

        let version = c.read_u32::<Endian>()?;
        if version != 1 || version != 2 {
            return_mmvdumperror!("Invalid version number");
        }

        let gen1 = c.read_i64::<Endian>()?;
        let gen2 = c.read_i64::<Endian>()?;
        if gen1 != gen2 {
            return_mmvdumperror!("Generation timestamps don't match");
        } 

        let toc_count = c.read_u32::<Endian>()?;
        if toc_count > 5 || toc_count < 2 {
            return_mmvdumperror!("Invalid TOC count");
        }

        let flags = c.read_u32::<Endian>()?;
        let pid = c.read_i32::<Endian>()?;

        let cluster_id = c.read_u32::<Endian>()?;
        if (cluster_id >> (32 - CLUSTER_ID_BIT_LEN)) != 0 {
            return_mmvdumperror!("Invalid cluster ID");
        }

        Ok(Header {
            _mmv_offset: _mmv_offset,
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

pub struct TOC {
    _mmv_offset: u64,
    sec: u32,
    entries: u32,
    sec_offset: u64
}

impl TOC {
    fn from_cursor(c: &mut Cursor<Vec<u8>>) -> Result<Self, MMVDumpError> {
        let _mmv_offset = c.position();

        let sec = c.read_u32::<Endian>()?;
        if sec > 5 {
            return_mmvdumperror!("Invalid TOC type");
        }

        let entries = c.read_u32::<Endian>()?;

        let sec_offset = c.read_u64::<Endian>()?;
        if sec_offset == 0 {
            return_mmvdumperror!("Invalid section offset");
        }

        Ok(TOC {
            _mmv_offset: _mmv_offset,
            sec: sec,
            entries: entries,
            sec_offset: sec_offset
        })
    }
}

pub struct MetricBlk {
    _mmv_offset: u64,
    name: [u8; METRIC_NAME_MAX_LEN as usize],
    item: u32,
    typ: u32,
    sem: u32,
    unit: u32,
    indom: u32,
    pad: u32,
    short_help_offset: u64,
    long_help_offset: u64
}

impl MetricBlk {
    fn from_cursor(c: &mut Cursor<Vec<u8>>) -> Result<Self, MMVDumpError> {
        let _mmv_offset = c.position();

        let mut name = [0; METRIC_NAME_MAX_LEN as usize];
        c.read_exact(&mut name)?;

        let item = c.read_u32::<Endian>()?;
        let typ = c.read_u32::<Endian>()?;
        let sem = c.read_u32::<Endian>()?;
        let unit = c.read_u32::<Endian>()?;
        let indom = c.read_u32::<Endian>()?;

        let pad = c.read_u32::<Endian>()?;
        if pad != 0 {
            return_mmvdumperror!("Invalid pad bytes");
        }

        let short_help_offset = c.read_u64::<Endian>()?;
        if short_help_offset == 0 {
            return_mmvdumperror!("Invalid short help offset");
        }

        let long_help_offset = c.read_u64::<Endian>()?;
        if long_help_offset == 0 {
            return_mmvdumperror!("Invalid long help offset");
        }

        Ok(MetricBlk {
            _mmv_offset: _mmv_offset,
            name: name,
            item: item,
            typ: typ,
            sem: sem,
            unit: unit,
            indom: indom,
            pad: pad,
            short_help_offset: short_help_offset,
            long_help_offset: long_help_offset
        })
    }
}

pub struct ValueBlk {
    _mmv_offset: u64,
    value: u64,
    string_offset: u64,
    metric_offset: u64,
    instance_offset: u64
}

impl ValueBlk {
    fn from_cursor(c: &mut Cursor<Vec<u8>>) -> Result<Self, MMVDumpError> {
        let _mmv_offset = c.position();
        let value = c.read_u64::<Endian>()?;

        let string_offset = c.read_u64::<Endian>()?;
        if string_offset == 0 {
            return_mmvdumperror!("Invalid string offset");
        }

        let metric_offset = c.read_u64::<Endian>()?;
        if metric_offset == 0 {
            return_mmvdumperror!("Invalid metric offset");
        }

        let instance_offset = c.read_u64::<Endian>()?;
        if instance_offset == 0 {
            return_mmvdumperror!("Invalid instance offset");
        }

        Ok(ValueBlk {
            _mmv_offset: _mmv_offset,
            value: value,
            string_offset: string_offset,
            metric_offset: metric_offset,
            instance_offset: instance_offset
        })
    }
}

pub struct IndomBlk {
    _mmv_offset: u64,
    indom: u32,
    instances: u32,
    instances_offset: u64,
    short_help_offset: u64,
    long_help_offset: u64
}

impl IndomBlk {
    fn from_cursor(c: &mut Cursor<Vec<u8>>) -> Result<Self, MMVDumpError> {
        let _mmv_offset = c.position();
        let indom = c.read_u32::<Endian>()?;
        let instances = c.read_u32::<Endian>()?;

        let instances_offset = c.read_u64::<Endian>()?;
        if instances_offset == 0 {
            return_mmvdumperror!("Invalid instance offset");
        }

        let short_help_offset = c.read_u64::<Endian>()?;
        if short_help_offset == 0 {
            return_mmvdumperror!("Invalid short help offset");
        }

        let long_help_offset = c.read_u64::<Endian>()?;
        if long_help_offset == 0 {
            return_mmvdumperror!("Invalid long help offset");
        }

        Ok(IndomBlk {
            _mmv_offset: _mmv_offset,
            indom: indom,
            instances: instances,
            instances_offset: instances_offset,
            short_help_offset: short_help_offset,
            long_help_offset: long_help_offset
        })
    }
}

pub struct InstanceBlk {
    _mmv_offset: u64,
    indom_offset: u64,
    pad: u32,
    internal_id: u32,
    external_id: [u8; METRIC_NAME_MAX_LEN as usize]
}

impl InstanceBlk {
    fn from_cursor(c: &mut Cursor<Vec<u8>>) -> Result<Self, MMVDumpError> {
        let _mmv_offset = c.position();

        let indom_offset = c.read_u64::<Endian>()?;
        if indom_offset == 0 {
            return_mmvdumperror!("Invalid indom offset");
        }

        let pad = c.read_u32::<Endian>()?;
        if pad != 0 {
            return_mmvdumperror!("Invalid pad bytes");
        }

        let internal_id = c.read_u32::<Endian>()?;

        let mut external_id = [0; METRIC_NAME_MAX_LEN as usize];
        c.read_exact(&mut external_id)?;

        Ok(InstanceBlk {
            _mmv_offset: _mmv_offset,
            indom_offset: indom_offset,
            pad: pad,
            internal_id: internal_id,
            external_id: external_id
        })
    }
}

pub struct StringBlk {
    _mmv_offset: u64,
    string: [u8; STRING_BLOCK_LEN as usize]
}

impl StringBlk {
    fn from_cursor(c: &mut Cursor<Vec<u8>>) -> Result<Self, MMVDumpError> {
        let _mmv_offset = c.position();

        let mut string = [0; STRING_BLOCK_LEN as usize];
        c.read_exact(&mut string)?;

        Ok(StringBlk {
            _mmv_offset: _mmv_offset,
            string: string
        })
    }
}

macro_rules! blks_from_toc (
    ($toc:expr, $blk_typ:tt, $cursor:expr) => (
        if let Some(ref toc) = $toc {
            let mut blks = Vec::with_capacity(toc.entries as usize);

            $cursor.set_position(toc.sec_offset);
            for _ in 0..toc.entries as usize {
                blks.push($blk_typ::from_cursor(&mut $cursor)?);
            }

            blks
        } else {
            Vec::new()
        }
    )
);

pub fn dump(mmv_path: &Path) -> Result<MMV, MMVDumpError> {
    let mut mmv_bytes = Vec::new();
    let mut file = File::open(mmv_path)?;
    file.read_to_end(&mut mmv_bytes)?;

    let mut cursor = Cursor::new(mmv_bytes);
    
    let hdr = Header::from_cursor(&mut cursor)?;

    let mut indom_toc = None;
    let mut instance_toc = None;
    let mut metric_toc = None;
    let mut value_toc = None;
    let mut string_toc = None;

    for _ in 0..hdr.toc_count {
        let toc = TOC::from_cursor(&mut cursor)?;
        if toc.sec == INDOM_TOC_CODE { indom_toc = Some(toc); }
        else if toc.sec == INSTANCE_TOC_CODE { instance_toc = Some(toc); }
        else if toc.sec == METRIC_TOC_CODE { metric_toc = Some(toc); }
        else if toc.sec == VALUES_TOC_CODE { value_toc = Some(toc); }
        else if toc.sec == STRINGS_TOC_CODE { string_toc = Some(toc); }
    }

    if metric_toc.is_none() {
        return_mmvdumperror!("Metric TOC absent");
    }
    if value_toc.is_none() {
        return_mmvdumperror!("String TOC absent");
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
