use byteorder::WriteBytesExt;
use memmap::{Mmap, MmapViewSync, Protection};
use std::collections::hash_map::DefaultHasher;
use std::fmt;
use std::hash::Hasher;
use std::io;
use std::io::{Cursor, Write};
use std::mem;

const ITEM_BIT_LEN: usize = 10;

pub (super) enum MTCode {
    I32 = 0,
    U32,
    I64,
    U64,
    F32,
    F64,
    String
}

mod private {
    use byteorder::WriteBytesExt;
    use std::io;

    /// Generic type for any Metric's value
    pub trait MetricType {
        private_decl!{}

        /// Returns the MMV metric type code
        fn type_code(&self) -> u32;
        /// Writes the byte representation of the value to a writer.
        ///
        /// For integer and float types, the byte sequence is little endian.
        ///
        /// For the string type, the UTF-8 byte sequence is suffixed with a null byte.
        fn write_to_writer<W: WriteBytesExt>(&self, writer: &mut W) -> io::Result<()>;
    }
}

pub (super) use self::private::MetricType;

macro_rules! impl_metric_type_for (
    ($typ:tt, $base_typ:tt, $type_code:expr) => (
        impl MetricType for $typ {

            private_impl!{}

            fn type_code(&self) -> u32 {
                $type_code as u32
            }

            fn write_to_writer<W: WriteBytesExt>(&self, w: &mut W)
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

impl_metric_type_for!(i32, u32, MTCode::I32);
impl_metric_type_for!(u32, u32, MTCode::U32);
impl_metric_type_for!(i64, u64, MTCode::I64);
impl_metric_type_for!(u64, u64, MTCode::U64);
impl_metric_type_for!(f32, u32, MTCode::F32);
impl_metric_type_for!(f64, u64, MTCode::F64);

impl MetricType for String {
    private_impl!{}

    fn type_code(&self) -> u32 {
        MTCode::String as u32
    }

    fn write_to_writer<W: Write>(&self, writer: &mut W) -> io::Result<()> {
        writer.write_all(self.as_bytes())?;
        writer.write_all(&[0])
    }
}

#[derive(Copy, Clone)]
/// Scale for the space component of a unit
pub enum Space {
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
pub enum Time {
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
pub enum Count {
    One = 0
}

#[derive(Copy, Clone)]
/// Unit for a Metric
pub struct Unit {
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
    pmapi_repr: u32
}

macro_rules! check_dim (
    ($dim:expr) => (
        if $dim > 7 || $dim < -8 {
            return Err(format!("Unit dimension {} is out of range [-8, 7]", $dim))
        }
    )
);

impl Unit {
    /// Returns an empty unit with all dimensions set to `0`
    /// and all scales set to an undefined variant
    pub fn new() -> Self {
        Unit {
            pmapi_repr: 0
        }
    }

    /// Modifies and returns the unit with given space scale and dimension
    pub fn space(mut self, scale: Space, dim: i8) -> Result<Self, String> {
        check_dim!(dim);
        self.pmapi_repr |= (scale as u32) << 16;
        self.pmapi_repr |= ((dim as u32) & ((1 << 4) - 1)) << 28;
        Ok(self)
    }

    /// Modifies and returns the unit with given time scale and dimension
    pub fn time(mut self, time: Time, dim: i8) -> Result<Self, String> {
        check_dim!(dim);
        self.pmapi_repr |= (time as u32) << 12;
        self.pmapi_repr |= ((dim as u32) & ((1 << 4) - 1)) << 24;
        Ok(self)
    }

    /// Modifies and returns the unit with given count scale and dimension
    pub fn count(mut self, count: Count, dim: i8) -> Result<Self, String> {
        check_dim!(dim);
        self.pmapi_repr |= (count as u32) << 8;
        self.pmapi_repr |= ((dim as u32) & ((1 << 4) - 1)) << 20;
        Ok(self)
    }
}

#[derive(Copy, Clone)]
/// Semantic for a Metric
pub enum Semantics {
    /// Counter
    Counter  = 1,
    /// Instant
    Instant  = 3,
    /// Discrete
    Discrete = 4
}

impl fmt::Display for Semantics {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Semantics::Counter => write!(f, "counter")?,
            Semantics::Instant => write!(f, "instant")?,
            Semantics::Discrete => write!(f, "discrete")?
        }
        write!(f, " (0x{:x})", *self as u32)
    }
}

/// Singleton metric
pub struct Metric<T> {
    name: String,
    item: u32,
    sem: Semantics,
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
    /// Creates a new PCP MMV Metric.
    ///
    /// The value type for the metric is determined and fixed
    /// at compile time.
    ///
    /// `name` length should not exceed 63 bytes
    ///
    /// `shorthelp_text` length should not exceed 255 bytes
    ///
    /// `longhelp_text` length should not exceed 255 bytes
    pub fn new(
        name: &str, init_val: T, sem: Semantics, unit: Unit, 
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

        let mut hasher = DefaultHasher::new();
        hasher.write(name.as_bytes());
        let item = (hasher.finish() as u32) & ((1 << ITEM_BIT_LEN) - 1);

        Ok(Metric {
            name: name.to_owned(),
            item: item,
            sem: sem,
            indom: 0,
            unit: unit.pmapi_repr,
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
    
    pub fn name(&self) -> &str { &self.name }
    pub fn item(&self) -> u32 { self.item }
    pub fn type_code(&self) -> u32 { self.val.type_code() }
    pub fn sem(&self) -> &Semantics { &self.sem }
    pub fn unit(&self) -> u32 { self.unit }
    pub fn indom(&self) -> u32 { self.indom }
    pub fn shorthelp(&self) -> &str { &self.shorthelp }
    pub fn longhelp(&self) -> &str { &self.longhelp }

    pub (super) fn set_mmap_view(&mut self, mmap_view: MmapViewSync) {
        self.mmap_view = mmap_view;
    }

    pub (super) fn write_val(&self, cursor: &mut Cursor<&mut [u8]>) -> io::Result<()> {
        self.val.write_to_writer(cursor)
    }
}

#[test]
fn test_units() {
    assert_eq!(Unit::new().pmapi_repr, 0);

    assert_eq!(
        Unit::new().space(Space::KByte, 1).unwrap().pmapi_repr,
        1 << 28 | (Space::KByte as u32) << 16
    );
    assert_eq!(
        Unit::new().time(Time::Min, 1).unwrap().pmapi_repr,
        1 << 24 | (Time::Min as u32) << 12
    );
    assert_eq!(
        Unit::new().count(Count::One, 1).unwrap().pmapi_repr,
        1 << 20 | (Count::One as u32) << 8
    );

    let (space_dim, time_dim, count_dim) = (-3, -2, 1);
    let unit = Unit::new()
        .space(Space::EByte, space_dim).unwrap()
        .time(Time::Hour, time_dim).unwrap()
        .count(Count::One, count_dim).unwrap();

    assert_eq!(unit.pmapi_repr,
        ((space_dim as u32) & ((1 << 4) - 1)) << 28 |
        ((time_dim as u32) & ((1 << 4) - 1)) << 24 |
        ((count_dim as u32) & ((1 << 4) - 1)) << 20 |
        (Space::EByte as u32) << 16 |
        (Time::Hour as u32) << 12 |
        (Count::One as u32) << 8
    );

    assert!(Unit::new().space(Space::Byte, 8).is_err());
    assert!(Unit::new().time(Time::Sec, -9).is_err());
}

#[test]
fn test_invalid_metric_strings() {
    use rand::{thread_rng, Rng};

    let invalid_name: String = thread_rng().gen_ascii_chars()
        .take(super::METRIC_NAME_MAX_LEN as usize).collect();
    let m1 = Metric::new(
        &invalid_name,
        0, Semantics::Discrete, Unit::new(), "", "",
    );
    assert!(m1.is_err());

    let invalid_shorthelp: String = thread_rng().gen_ascii_chars()
        .take(super::STRING_BLOCK_LEN as usize).collect();
    let m2 = Metric::new(
        "", 0, Semantics::Discrete, Unit::new(),
        &invalid_shorthelp,
        "",
    );
    assert!(m2.is_err());

    let invalid_longhelp: String = thread_rng().gen_ascii_chars()
        .take(super::STRING_BLOCK_LEN as usize).collect();
    let m3 = Metric::new(
        "", 0, Semantics::Discrete, Unit::new(), "",
        &invalid_longhelp,
    );
    assert!(m3.is_err());
}

#[test]
fn test_random_numeric_metrics() {
    use byteorder::ReadBytesExt;
    use rand::{thread_rng, Rng};
    use super::Client;

    let mut metrics = Vec::new();
    let mut new_vals = Vec::new();
    let n_metrics = thread_rng().gen::<u8>() % 20;

    let mut client = Client::new("numeric_metrics").unwrap();
    client.begin(n_metrics as u64).unwrap();
    
    for _ in 1..n_metrics {
        let rnd_name: String = thread_rng().gen_ascii_chars()
            .take(super::METRIC_NAME_MAX_LEN as usize - 1).collect();

        let rnd_shorthelp: String = thread_rng().gen_ascii_chars()
            .take(super::STRING_BLOCK_LEN as usize - 1).collect();

        let rnd_longhelp: String = thread_rng().gen_ascii_chars()
            .take(super::STRING_BLOCK_LEN as usize - 1).collect();

        let rnd_val1 = thread_rng().gen::<u32>();

        let mut metric = Metric::new(
            &rnd_name,
            rnd_val1,
            Semantics::Discrete,
            Unit::new(),
            &rnd_shorthelp,
            &rnd_longhelp,
        ).unwrap();

        assert_eq!(metric.val(), rnd_val1);

        let rnd_val2 = thread_rng().gen::<u32>();
        assert!(metric.set_val(rnd_val2).is_ok());
        assert_eq!(metric.val(), rnd_val2);

        client.register_metric(&mut metric).unwrap();

        metrics.push(metric);
        new_vals.push(thread_rng().gen::<u32>());
    }

    client.export().unwrap();

    for (m, v) in metrics.iter_mut().zip(&mut new_vals) {
        assert!(m.set_val(*v).is_ok());
    }

    for (m, v) in metrics.iter_mut().zip(new_vals) {
        let mut slice = unsafe { m.mmap_view.as_slice() };
        assert_eq!(v, slice.read_u64::<super::Endian>().unwrap() as u32);
    }
}

#[test]
fn test_simple_metrics() {
    use byteorder::ReadBytesExt;
    use rand::{thread_rng, Rng};
    use std::ffi::CStr;
    use std::mem::transmute;
    use super::Client;

    // f64 metric
    let hz = Unit::new().time(Time::Sec, -1).unwrap();
    let mut freq = Metric::new(
        "frequency",
        thread_rng().gen::<f64>(),
        Semantics::Instant,
        hz,
        "", "",
    ).unwrap();

    // string metric
    let mut color = Metric::new(
        "color",
        String::from("cyan"),
        Semantics::Discrete,
        Unit::new(),
        "Color", "",
    ).unwrap();

    // u32 metric
    let mut photons = Metric::new(
        "photons",
        thread_rng().gen::<u32>(),
        Semantics::Counter,
        Unit::new().count(Count::One, 1).unwrap(),
        "No. of photons",
        "Number of photons emitted by source",
    ).unwrap();

    Client::new("physical_metrics").unwrap()
        .begin(3).unwrap()
        .register_metric(&mut freq).unwrap()
        .register_metric(&mut color).unwrap()
        .register_metric(&mut photons).unwrap()
        .export().unwrap();

    let new_freq = thread_rng().gen::<f64>();
    assert!(freq.set_val(new_freq).is_ok());

    let new_color = String::from("magenta");
    assert!(color.set_val(new_color.clone()).is_ok());

    let new_photon_count = thread_rng().gen::<u32>();
    assert!(photons.set_val(new_photon_count).is_ok());

    let mut freq_slice = unsafe { freq.mmap_view.as_slice() };
    assert_eq!(
        new_freq,
        unsafe { 
            transmute::<u64, f64>(freq_slice.read_u64::<super::Endian>().unwrap())
        }
    );

    let color_slice = unsafe { color.mmap_view.as_slice() };
    let cstr = unsafe {
        CStr::from_ptr(color_slice.as_ptr() as *const i8)
    };
    assert_eq!(new_color, cstr.to_str().unwrap());

    let mut photon_slice = unsafe { photons.mmap_view.as_slice() };
    assert_eq!(
        new_photon_count,
        photon_slice.read_u64::<super::Endian>().unwrap() as u32
    );

    // TODO: after implementing mmvdump functionality, test the
    // bytes of the entier MMV file
}
