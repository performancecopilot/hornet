use byteorder::WriteBytesExt;
use memmap::{Mmap, MmapViewSync, Protection};
use std::ffi::CString;
use std::io;
use std::io::{Cursor, Write};
use std::mem;

const ITEM_BIT_LEN: usize = 10;

pub const I32_METRIC_TYPE_CODE: u32 = 0;
pub const U32_METRIC_TYPE_CODE: u32 = 1;
pub const I64_METRIC_TYPE_CODE: u32 = 2;
pub const U64_METRIC_TYPE_CODE: u32 = 3;
pub const F32_METRIC_TYPE_CODE: u32 = 4;
pub const F64_METRIC_TYPE_CODE: u32 = 5;
pub const STRING_METRIC_TYPE_CODE: u32 = 6;

/// Generic type for any Metric's value
pub trait MetricType {
    /// Returns the MMV metric type code
    fn type_code(&self) -> u32;
    /// Writes the byte representation of the value to a writer.
    ///
    /// For integer and float types, the byte sequence is little endian.
    ///
    /// For the string type, the UTF-8 byte sequence is suffixed with a null byte.
    fn write_to_writer<W: WriteBytesExt>(&self, writer: &mut W)
        -> io::Result<()>;
}

macro_rules! impl_metric_type_for (
    ($typ:tt, $base_typ:tt, $type_code:expr) => (
        impl MetricType for $typ {
            
            fn type_code(&self) -> u32 {
                $type_code
            }

            fn write_to_writer<W: WriteBytesExt>(&self, mut w: &mut W)
            -> io::Result<()> {
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

    fn write_to_writer<W: Write>(&self, mut writer: &mut W) -> io::Result<()> {
        writer.write_all(CString::new(self.as_str())?.to_bytes_with_nul())
    }
}

#[derive(Copy, Clone)]
/// Scale for the space component of a unit
pub enum SpaceScale {
    /// byte
    Byte = 0,
    /// kilobyte (1024 bytes) 
    KByte,
    /// megabyte (1024^2 bytes)
    MByte,
    /// gigabyte (1024^3 bytes)
    GByte,
    /// terabyte (1024^4 bytes)
    TByte,
    /// petabyte (1024^5 bytes)
    PByte,
    /// exabyte (1024^6 bytes)
    EByte
}

#[derive(Copy, Clone)]
/// Scale for the time component of a unit
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
/// Scale for the count component of a unit
pub enum CountScale {
    One = 0
}

#[derive(Copy, Clone)]
/// Unit for a Metric
pub struct Unit {
    /// Space scale
    pub space_scale: SpaceScale,
    /// Time scale
    pub time_scale: TimeScale,
    /// Count scale
    pub count_scale: CountScale,
    /// Space dimension (uses least-significant 4 bits only)
    pub space_dim: i8,
    /// Time dimension (uses least-significant 4 bits only)
    pub time_dim: i8,
    /// Count dimension (uses least-significant 4 bits only)
    pub count_dim: i8
}

lazy_static! {
    static ref SPACE_UNIT: Unit = {
        let mut unit = Unit::empty();
        unit.space_dim = 1;
        unit
    };

    static ref TIME_UNIT: Unit = {
        let mut unit = Unit::empty();
        unit.time_dim = 1;
        unit
    };

    static ref COUNT_UNIT: Unit = {
        let mut unit = Unit::empty();
        unit.count_dim = 1;
        unit
    };
}

impl Unit {
    /// Returns an empty unit with all dimensions set to `0`
    /// and all scales set to an undefined variant
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

    /// Returns a unit with space dimension `1` and given
    /// space scale
    pub fn space(space_scale: SpaceScale) -> Self {
        let mut unit = SPACE_UNIT.clone();
        unit.space_scale = space_scale;
        unit
    }

    /// Returns a unit with time dimension `1` and given
    /// time scale
    pub fn time(time_scale: TimeScale) -> Self {
        let mut unit = TIME_UNIT.clone();
        unit.time_scale = time_scale;
        unit
    }

    /// Returns a unit with count dimension `1` and given
    /// count scale
    pub fn count() -> Self {
        COUNT_UNIT.clone()
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
/// Semantic for a Metric
pub enum Semamtics {
    /// Counter
    Counter  = 1,
    /// Instant
    Instant  = 3,
    /// Discrete
    Discrete = 4
}

/// Singleton metric
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
    /// Creates a new Metric with necessary attributes for a
    /// PCP MMV metric.
    ///
    /// The value type for the metric is determined and fixed
    /// at compile time.
    ///
    /// `item` is the unique metric ID component of the PMID. Only
    /// the least-significant 10 bits are used.
    pub fn new(
        name: &str, item: u32, sem: Semamtics,
        unit: Unit, init_val: T,
        shorthelp_text: &str, longhelp_text: &str) -> Result<Self, String> {
        
        if name.len() >= super::METRIC_NAME_MAX_LEN as usize {
            return Err(format!("name longer than {} bytes", super::METRIC_NAME_MAX_LEN - 1));
        }
        if shorthelp_text.len() >= super::STRING_BLOCK_LEN as usize {
            return Err(format!("short help text longer than {} bytes", super::STRING_BLOCK_LEN - 1));
        }
        if longhelp_text.len() >= super::STRING_BLOCK_LEN as usize {
            return Err(format!("long help text longer than {} bytes", super::STRING_BLOCK_LEN - 1));
        }

        Ok(Metric {
            name: name.to_owned(),
            item: item & ((1 << ITEM_BIT_LEN) - 1),
            sem: sem,
            indom: 0,
            unit: unit.pmapi_repr(),
            shorthelp: shorthelp_text.to_owned(),
            longhelp: longhelp_text.to_owned(),
            val: init_val,
            mmap_view: unsafe { SCRATCH_VIEW.clone() }
        })
    }

    /// Returns the current value of the metric
    pub fn val(&self) -> T {
        self.val.clone()
    }

    /// Sets the current value of the metric.
    ///
    /// If the metric is exported using a client,
    /// the value is written to the relevant MMV file.
    ///
    /// If the metric isn't exported, this method will still
    /// succeed and update the value.
    pub fn set_val(&mut self, new_val: T) -> io::Result<()> {
        new_val.write_to_writer(unsafe { &mut self.mmap_view.as_mut_slice() })?;
        self.val = new_val;
        Ok(())
    }
}

/// PCP MMV Metric
///
/// Useful for dealing with collections of Metrics with different
/// value types, and also implementing custom MMV writers.
pub trait MMVMetric {
    fn name(&self) -> &str;
    fn item(&self) -> u32;
    fn type_code(&self) -> u32;
    fn sem(&self) -> &Semamtics;
    fn unit(&self) -> u32;
    fn indom(&self) -> u32;
    fn shorthelp(&self) -> &str;
    fn longhelp(&self) -> &str;
    fn write_val(&mut self, cursor: &mut Cursor<&mut [u8]>) -> io::Result<()>;
    unsafe fn mmap_view(&mut self) -> MmapViewSync;
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

    fn write_val(&mut self, cursor: &mut Cursor<&mut [u8]>) -> io::Result<()> {
        self.val.write_to_writer(cursor)
    }

    unsafe fn mmap_view(&mut self) -> MmapViewSync {
        self.mmap_view.clone()
    }

    fn set_mmap_view(&mut self, mmap_view: MmapViewSync) {
        self.mmap_view = mmap_view;
    }
}

#[test]
fn test_units() {
    assert_eq!(Unit::empty().pmapi_repr(), 0);

    assert_eq!(SPACE_UNIT.pmapi_repr(), 1 << 28);
    assert_eq!(TIME_UNIT.pmapi_repr(), 1 << 24);
    assert_eq!(COUNT_UNIT.pmapi_repr(), 1 << 20);

    assert_eq!(
        Unit::space(SpaceScale::KByte).pmapi_repr(),
        1 << 28 | (SpaceScale::KByte as u32) << 16
    );
    assert_eq!(
        Unit::time(TimeScale::Min).pmapi_repr(),
        1 << 24 | (TimeScale::Min as u32) << 12
    );
    assert_eq!(
        Unit::count().pmapi_repr(),
        1 << 20 | (CountScale::One as u32) << 8
    );

    let (space_dim, time_dim, count_dim) = (-3, -2, 1);
    let unit = Unit {
        space_scale: SpaceScale::EByte,
        time_scale: TimeScale::Hour,
        count_scale: CountScale::One,
        space_dim: space_dim,
        time_dim: time_dim,
        count_dim: count_dim
    };
    assert_eq!(
        unit.pmapi_repr(),
        ((space_dim as u32) & ((1 << 4) - 1)) << 28 |
        ((time_dim as u32) & ((1 << 4) - 1)) << 24 |
        ((count_dim as u32) & ((1 << 4) - 1)) << 20 |
        (SpaceScale::EByte as u32) << 16 |
        (TimeScale::Hour as u32) << 12 |
        (CountScale::One as u32) << 8
    );
}

#[test]
fn test_invalid_metric_strings() {
    use rand::{thread_rng, Rng};

    let invalid_name: String = thread_rng().gen_ascii_chars()
        .take(super::METRIC_NAME_MAX_LEN as usize).collect();
    let m1 = Metric::new(
        &invalid_name,
        0, Semamtics::Counter, Unit::count(), 0, "", "",
    );
    assert!(m1.is_err());

    let invalid_shorthelp: String = thread_rng().gen_ascii_chars()
        .take(super::STRING_BLOCK_LEN as usize).collect();
    let m2 = Metric::new(
        "", 0, Semamtics::Counter, Unit::count(), 0,
        &invalid_shorthelp,
        "",
    );
    assert!(m2.is_err());

    let invalid_longhelp: String = thread_rng().gen_ascii_chars()
        .take(super::STRING_BLOCK_LEN as usize).collect();
    let m3 = Metric::new(
        "", 0, Semamtics::Counter, Unit::count(), 0, "",
        &invalid_longhelp,
    );
    assert!(m3.is_err());

}

#[test]
fn test_numeric_metrics() {
    use rand::{thread_rng, Rng};
    use byteorder::ReadBytesExt;

    let mut metrics = Vec::new();
    let n_metrics = thread_rng().gen::<u8>() % 20;
    for _ in 1..n_metrics {
        let rnd_name: String = thread_rng().gen_ascii_chars()
            .take(super::METRIC_NAME_MAX_LEN as usize - 1).collect();

        let rnd_shorthelp: String = thread_rng().gen_ascii_chars()
            .take(super::STRING_BLOCK_LEN as usize - 1).collect();

        let rnd_longhelp: String = thread_rng().gen_ascii_chars()
            .take(super::STRING_BLOCK_LEN as usize - 1).collect();

        let rnd_item = thread_rng().gen::<u32>();
        let rnd_val1 = thread_rng().gen::<u32>();

        let mut metric = Metric::new(
            &rnd_name,
            rnd_item,
            Semamtics::Counter,
            Unit::count(),
            rnd_val1,
            &rnd_shorthelp,
            &rnd_longhelp,
        ).unwrap();

        assert_eq!(metric.val(), rnd_val1);

        let rnd_val2 = thread_rng().gen::<u32>();
        assert!(metric.set_val(rnd_val2).is_ok());
        assert_eq!(metric.val(), rnd_val2);
        
        metrics.push(metric);
    }

    {
        let mut mmv_metrics: Vec<&mut MMVMetric> =
            metrics.iter_mut().map(|m| m as &mut MMVMetric).collect();
        let client = super::Client::new("metrics").unwrap();
        client.export(&mut mmv_metrics).unwrap();
    }

    for m in metrics.iter_mut() {
        let rnd_val = thread_rng().gen::<u32>();
        assert!(m.set_val(rnd_val).is_ok());

        let mut slice = unsafe { m.mmap_view.as_slice() };
        assert_eq!(m.val(), slice.read_u64::<super::Endian>().unwrap() as u32);
    }
}

#[test]
fn test_string_metrics() {
    use rand::{thread_rng, Rng};
    use std::ffi::CStr;

    let mut metrics = Vec::new();
    let n_metrics = thread_rng().gen::<u8>() % 20;
    for _ in 1..n_metrics {
        let rnd_name: String = thread_rng().gen_ascii_chars()
            .take(super::METRIC_NAME_MAX_LEN as usize - 1).collect();

        let rnd_shorthelp: String = thread_rng().gen_ascii_chars()
            .take(super::STRING_BLOCK_LEN as usize - 1).collect();

        let rnd_longhelp: String = thread_rng().gen_ascii_chars()
            .take(super::STRING_BLOCK_LEN as usize - 1).collect();

        let rnd_item = thread_rng().gen::<u32>();
        let rnd_val1: String = thread_rng().gen_ascii_chars()
            .take(super::STRING_BLOCK_LEN as usize - 1).collect();

        let mut metric = Metric::new(
            &rnd_name,
            rnd_item,
            Semamtics::Counter,
            Unit::count(),
            rnd_val1.clone(),
            &rnd_shorthelp,
            &rnd_longhelp,
        ).unwrap();

        assert_eq!(metric.val(), rnd_val1);

        let rnd_val2: String = thread_rng().gen_ascii_chars()
            .take(super::STRING_BLOCK_LEN as usize - 1).collect();
        assert!(metric.set_val(rnd_val2.clone()).is_ok());
        assert_eq!(metric.val(), rnd_val2);
        
        metrics.push(metric);
    }

    {
        let mut mmv_metrics: Vec<&mut MMVMetric> =
            metrics.iter_mut().map(|m| m as &mut MMVMetric).collect();
        let client = super::Client::new("metrics").unwrap();
        client.export(&mut mmv_metrics).unwrap();
    }

    for m in metrics.iter_mut() {
        let rnd_val: String = thread_rng().gen_ascii_chars()
            .take(super::STRING_BLOCK_LEN as usize - 1).collect();
        assert!(m.set_val(rnd_val.clone()).is_ok());

        let slice = unsafe { m.mmap_view.as_slice() };
        let cstr = CStr::from_bytes_with_nul(slice).unwrap();
        assert_eq!(m.val(), cstr.to_str().unwrap());
    }
}
