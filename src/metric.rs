use byteorder::WriteBytesExt;
use memmap::{Mmap, MmapViewSync, Protection};
use std::ffi::CString;
use std::io;
use std::io::Cursor;
use std::mem;

const ITEM_BIT_LEN: usize = 10;

pub const I32_METRIC_TYPE_CODE: u32 = 0;
pub const U32_METRIC_TYPE_CODE: u32 = 1;
pub const I64_METRIC_TYPE_CODE: u32 = 2;
pub const U64_METRIC_TYPE_CODE: u32 = 3;
pub const F32_METRIC_TYPE_CODE: u32 = 4;
pub const F64_METRIC_TYPE_CODE: u32 = 5;
pub const STRING_METRIC_TYPE_CODE: u32 = 6;

pub trait MetricType {
    fn type_code(&self) -> u32;
    fn write_to_writer<W: WriteBytesExt>(&self, writer: &mut W) -> io::Result<()>;
}

macro_rules! impl_metric_type_for (
    ($typ:tt, $base_typ:tt, $type_code:expr) => (
        impl MetricType for $typ {
            
            fn type_code(&self) -> u32 {
                $type_code
            }

            fn write_to_writer<W: WriteBytesExt>(&self, mut w: &mut W) -> io::Result<()> {
                w.write_u64::<super::Endian>(
                    unsafe {
                        mem::transmute::<$typ, $base_typ>(*self) as u64
                    }
                )
            }

        }
    )
);

impl_metric_type_for!(i32, u32, I32_METRIC_TYPE_CODE);
impl_metric_type_for!(u32, u32, U32_METRIC_TYPE_CODE);
impl_metric_type_for!(i64, u64, I64_METRIC_TYPE_CODE);
impl_metric_type_for!(u64, u64, U64_METRIC_TYPE_CODE);
impl_metric_type_for!(f32, u32, F32_METRIC_TYPE_CODE);
impl_metric_type_for!(f64, u64, F64_METRIC_TYPE_CODE);

impl MetricType for String {
    fn type_code(&self) -> u32 {
        STRING_METRIC_TYPE_CODE
    }

    fn write_to_writer<W: WriteBytesExt>(&self, mut writer: &mut W) -> io::Result<()> {
        writer.write_all(CString::new(self.as_str())?.to_bytes_with_nul())
    }
}

#[derive(Copy, Clone)]
pub enum SpaceScale {
    /// byte
    Byte = 0,
    /// kilobyte (1024) 
    KByte,
    /// megabyte (1024^2)
    MByte,
    /// gigabyte (1024^3)
    GByte,
    /// terabyte (1024^4)
    TByte,
    /// petabyte (1024^5)
    PByte,
    /// exabyte (1024^6)
    EByte
}

#[derive(Copy, Clone)]
pub enum TimeScale {
    /// nanosecond
    NSec = 0,
    /// microsecond
    USec,
    /// millisecond
    MSec,
    /// second
    Sec,
    /// minute
    Min,
    /// hour
    Hour
}

#[derive(Copy, Clone)]
pub enum CountScale {
    One = 0
}


pub struct Unit {
    pub space_scale: SpaceScale,
    pub time_scale: TimeScale,
    pub count_scale: CountScale,
    pub space_dim: i8,
    pub time_dim: i8,
    pub count_dim: i8
}

impl Unit {
    pub fn empty() -> Self {
        Unit {
            space_scale: SpaceScale::Byte,
            time_scale: TimeScale::NSec,
            count_scale: CountScale::One,
            space_dim: 0,
            time_dim: 0,
            count_dim: 0
        }
    }

    pub fn space(space_scale: SpaceScale) -> Self {
        let mut unit = Self::empty();
        unit.space_scale = space_scale;
        unit.space_dim = 1;
        unit
    }

    pub fn time(time_scale: TimeScale) -> Self {
        let mut unit = Self::empty();
        unit.time_scale = time_scale;
        unit.time_dim = 1;
        unit
    }

    pub fn count() -> Self {
        let mut unit = Self::empty();
        unit.count_dim = 1;
        unit
    }

    /*
        pmapi representation of a unit. below, 31 refers to MSB
        bits 31 - 28 : space dim   (signed)
             27 - 24 : time dim    (signed)
             23 - 20 : count dim   (signed)
             19 - 16 : space scale (unsigned)
             15 - 12 : time scale  (unsigned)
             11 - 8  : count scale (unsigned)
              7 - 0  : zero pad
    */
    fn pmapi_repr(&self) -> u32 {
        let mut repr = 0;

        repr |= ((self.space_dim as i32) & ((1 << 4) - 1)) << 28;
        repr |= ((self.time_dim  as i32) & ((1 << 4) - 1)) << 24;
        repr |= ((self.count_dim as i32) & ((1 << 4) - 1)) << 20;

        repr |= (self.space_scale as i32) << 16;
        repr |= (self.time_scale  as i32) << 12;
        repr |= (self.count_scale as i32) << 8;

        repr as u32
    }
}

#[derive(Copy, Clone)]
pub enum Semamtics {
    Counter  = 1,
    Instant  = 3,
    Discrete = 4
}

pub struct Metric<T> {
    name: String,
    item: u32,
    sem: Semamtics,
    indom: u32,
    unit: u32,
    shorthelp: String,
    longhelp: String,
    val: T,
    mmap_view: MmapViewSync
}

lazy_static! {
    static ref SCRATCH_VIEW: MmapViewSync = {
        Mmap::anonymous(super::STRING_BLOCK_LEN as usize, Protection::ReadWrite).unwrap()
            .into_view_sync()
    };
}

impl<T: MetricType + Clone> Metric<T> {
    pub fn new(
        name: &str, item: u32, sem: Semamtics,
        unit: Unit, init_val: T,
        shorthelp: &str, longhelp: &str) -> Result<Self, String> {
        
        if name.len() >= super::METRIC_NAME_MAX_LEN as usize {
            return Err(format!("name longer than {} bytes", super::METRIC_NAME_MAX_LEN - 1));
        }
        if shorthelp.len() >= super::STRING_BLOCK_LEN as usize {
            return Err(format!("short help text longer than {} bytes", super::STRING_BLOCK_LEN - 1));
        }
        if longhelp.len() >= super::STRING_BLOCK_LEN as usize {
            return Err(format!("long help text longer than {} bytes", super::STRING_BLOCK_LEN - 1));
        }

        Ok(Metric {
            name: name.to_owned(),
            item: item & ((1 << ITEM_BIT_LEN) - 1),
            sem: sem,
            indom: 0,
            unit: unit.pmapi_repr(),
            shorthelp: shorthelp.to_owned(),
            longhelp: longhelp.to_owned(),
            val: init_val,
            mmap_view: unsafe { SCRATCH_VIEW.clone() }
        })
    }

    pub fn val(&self) -> T {
        self.val.clone()
    }

    pub fn set_val(&mut self, new_val: T) -> io::Result<()> {
        new_val.write_to_writer(unsafe { &mut self.mmap_view.as_mut_slice() })?;
        self.val = new_val;
        Ok(())
    }
}

pub trait MMVMetric {
    fn name(&self) -> &str;
    fn item(&self) -> u32;
    fn type_code(&self) -> u32;
    fn sem(&self) -> &Semamtics;
    fn unit(&self) -> u32;
    fn indom(&self) -> u32;
    fn shorthelp(&self) -> &str;
    fn longhelp(&self) -> &str;
    fn write_value(&mut self, cursor: &mut Cursor<&mut [u8]>) -> io::Result<()>;
    fn set_mmap_view(&mut self, mmap_view: MmapViewSync);
}

impl<T: MetricType> MMVMetric for Metric<T> {
    fn name(&self) -> &str { &self.name }

    fn item(&self) -> u32 { self.item }

    fn type_code(&self) -> u32 { self.val.type_code() }

    fn sem(&self) -> &Semamtics { &self.sem }

    fn unit(&self) -> u32 { self.unit }

    fn indom(&self) -> u32 { self.indom }

    fn shorthelp(&self) -> &str { &self.shorthelp }

    fn longhelp(&self) -> &str { &self.longhelp }

    fn write_value(&mut self, cursor: &mut Cursor<&mut [u8]>) -> io::Result<()> {
        self.val.write_to_writer(cursor)
    }

    fn set_mmap_view(&mut self, mmap_view: MmapViewSync) {
        self.mmap_view = mmap_view;
    }
}

#[test]
fn test_metric() {

    let mut hello_metric = Metric::new(
        "hello", 1,
        Semamtics::Counter,
        Unit::empty(),
        "Hello".to_owned(),
        "Hello metric",
        "Metric of value type string").unwrap();

    let mut pi_metric = Metric::new(
        "pi", 1,
        Semamtics::Instant,
        Unit::time(TimeScale::Sec),
        3.0,
        "Pi metric",
        "Metric of value type double").unwrap();

    hello_metric.set_val("".to_owned()).unwrap();
    assert_eq!(&hello_metric.val(), "");

    pi_metric.set_val(0.0).unwrap();
    assert_eq!(pi_metric.val(), 0.0);

    let client = super::Client::new("metrics").unwrap();
    client.export(&mut [&mut hello_metric, &mut pi_metric]).unwrap();

    hello_metric.set_val("Hello World".to_owned()).unwrap();
    assert_eq!(&hello_metric.val(), "Hello World");

    pi_metric.set_val(3.14).unwrap();
    assert_eq!(pi_metric.val(), 3.14);

}
