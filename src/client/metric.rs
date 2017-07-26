use byteorder::WriteBytesExt;
use memmap::{Mmap, MmapViewSync, Protection};
use std::collections::HashSet;
use std::collections::hash_map::{DefaultHasher, HashMap};
use std::fmt;
use std::hash::{Hash, Hasher};
use std::io;
use std::io::Write;
use std::mem;

use super::super::mmv::MTCode;
use super::super::{
    ITEM_BIT_LEN,
    INDOM_BIT_LEN,
    METRIC_NAME_MAX_LEN,
    STRING_BLOCK_LEN
};

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

impl Space {
    fn from_u8(x: u8) -> Option<Self> {
        match x {
            0 => Some(Space::Byte),
            1 => Some(Space::KByte),
            2 => Some(Space::MByte),
            3 => Some(Space::GByte),
            4 => Some(Space::TByte),
            5 => Some(Space::PByte),
            6 => Some(Space::EByte),
            _ => None
        }
    }
}

impl fmt::Display for Space {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Space::Byte => write!(f, "B"),
            Space::KByte => write!(f, "KiB"),
            Space::MByte => write!(f, "MiB"),
            Space::GByte => write!(f, "GiB"),
            Space::TByte => write!(f, "TiB"),
            Space::PByte => write!(f, "PiB"),
            Space::EByte => write!(f, "EiB")
        }
    }
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

impl Time {
    fn from_u8(x: u8) -> Option<Self> {
        match x {
            0 => Some(Time::NSec),
            1 => Some(Time::USec),
            2 => Some(Time::MSec),
            3 => Some(Time::Sec),
            4 => Some(Time::Min),
            5 => Some(Time::Hour),
            _ => None
        }
    }
}

impl fmt::Display for Time {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Time::NSec => write!(f, "nsec"),
            Time::USec => write!(f, "usec"),
            Time::MSec => write!(f, "msec"),
            Time::Sec => write!(f, "sec"),
            Time::Min => write!(f, "min"),
            Time::Hour => write!(f, "hr"),
        }
    }
}

#[derive(Copy, Clone)]
/// Scale for the count component of a unit
pub enum Count {
    One = 0
}

impl Count {
    fn from_u8(x: u8) -> Option<Self> {
        match x {
            0 => Some(Count::One),
            _ => None
        }
    }
}

impl fmt::Display for Count {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self {
            Count::One => write!(f, "count")
        }
    }
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

const SPACE_DIM_LSB: u8 = 28;
const TIME_DIM_LSB: u8 = 24;
const COUNT_DIM_LSB: u8 = 20;
const SPACE_SCALE_LSB: u8 = 16;
const TIME_SCALE_LSB: u8 = 12;
const COUNT_SCALE_LSB: u8 = 8;

const LS_FOUR_BIT_MASK: u32 = 0xF;

macro_rules! check_dim (
    ($dim:expr) => (
        if $dim > 7 || $dim < -8 {
            return Err(format!("Unit dimension {} is out of range [-8, 7]", $dim))
        }
    )
);

impl Unit {
    /// Returns a unit constructed from a raw PMAPI representation
    pub fn from_raw(pmapi_repr: u32) -> Self {
        Unit {
            pmapi_repr: pmapi_repr
        }
    }

    /// Returns an empty unit with all dimensions set to `0`
    /// and all scales set to an undefined variant
    pub fn new() -> Self {
        Self::from_raw(0)
    }

    /// Modifies and returns the unit with given space scale and dimension
    pub fn space(mut self, scale: Space, dim: i8) -> Result<Self, String> {
        check_dim!(dim);
        self.pmapi_repr |= (scale as u32) << SPACE_SCALE_LSB;
        self.pmapi_repr |= ((dim as u32) & LS_FOUR_BIT_MASK) << SPACE_DIM_LSB;
        Ok(self)
    }

    /// Modifies and returns the unit with given time scale and dimension
    pub fn time(mut self, time: Time, dim: i8) -> Result<Self, String> {
        check_dim!(dim);
        self.pmapi_repr |= (time as u32) << TIME_SCALE_LSB;
        self.pmapi_repr |= ((dim as u32) & LS_FOUR_BIT_MASK) << TIME_DIM_LSB;
        Ok(self)
    }

    /// Modifies and returns the unit with given count scale and dimension
    pub fn count(mut self, count: Count, dim: i8) -> Result<Self, String> {
        check_dim!(dim);
        self.pmapi_repr |= (count as u32) << COUNT_SCALE_LSB;
        self.pmapi_repr |= ((dim as u32) & LS_FOUR_BIT_MASK) << COUNT_DIM_LSB;
        Ok(self)
    }

    fn space_scale(&self) -> u8 {
        ((self.pmapi_repr >> SPACE_SCALE_LSB) & LS_FOUR_BIT_MASK) as u8
    }

    fn time_scale(&self) -> u8 {
        ((self.pmapi_repr >> TIME_SCALE_LSB) & LS_FOUR_BIT_MASK) as u8
    }

    fn count_scale(&self) -> u8 {
        ((self.pmapi_repr >> COUNT_SCALE_LSB) & LS_FOUR_BIT_MASK) as u8
    }

    /*
        We have a 4-bit value in two's complement form which we have to
        sign-extend to 8 bits. We first left shift our 4 bits in pmapi_repr
        to the most significant position, then do an arithmetic right shift
        to bring them to the least significant position in order to
        sign-extend the remaining 4 bits.

        In Rust, an arthimetic/logical right shift is performed depending on
        whether the integer is signed or unsigned. Hence, we cast our integer
        to an i32 before we right shift it.
    */
    fn dim(&self, lsb: u8) -> i8 {
        (
            ( self.pmapi_repr << (32 - (lsb + 4)) ) as i32
            >> 28
        ) as i8
    }

    fn space_dim(&self) -> i8 {
        self.dim(SPACE_DIM_LSB)
    }

    fn time_dim(&self) -> i8 {
        self.dim(TIME_DIM_LSB)
    }

    fn count_dim(&self) -> i8 {
        self.dim(COUNT_DIM_LSB)
    }
}

macro_rules! write_dim (
    ($dim:expr, $scale:expr, $scale_type:tt, $f:expr) => (
        if let Some(dim_scale) = $scale_type::from_u8($scale) {
            write!($f, "{}", dim_scale)?;
            if $dim > 1 {
                write!($f, "^{}", $dim.abs())?;
            }
            write!($f, " ")?;
        }
    )
);

impl fmt::Display for Unit {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let space_dim = self.space_dim();
        let space_scale = self.space_scale();
        let time_dim = self.time_dim();
        let time_scale = self.time_scale();
        let count_dim = self.count_dim();
        let count_scale = self.count_scale();

        if space_dim > 0 {
            write_dim!(space_dim, space_scale, Space, f);
        }
        if time_dim > 0 {
            write_dim!(time_dim, time_scale, Time, f);
        }
        if count_dim > 0 {
            write_dim!(count_dim, count_scale, Count, f);
        }

        if space_dim < 0 || time_dim < 0 || count_dim < 0 {
            write!(f, " / ")?;
            if space_dim < 0 {
                write_dim!(space_dim, space_scale, Space, f);
            }
            if time_dim < 0 {
                write_dim!(time_dim, time_scale, Time, f);
            }
            if count_dim < 0 {
                write_dim!(count_dim, count_scale, Count, f);
            }
        }

        write!(f, "(0x{:x})", self.pmapi_repr)
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

impl Semantics {
    pub fn from_u32(x: u32) -> Option<Self> {
        match x {
            1 => Some(Semantics::Counter),
            3 => Some(Semantics::Instant),
            4 => Some(Semantics::Discrete),
            _ => None
        }
    }
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
    pub (super) val: T,
    pub (super) mmap_view: MmapViewSync
}

lazy_static! {
    static ref SCRATCH_VIEW: MmapViewSync = {
        Mmap::anonymous(STRING_BLOCK_LEN as usize, Protection::ReadWrite).unwrap()
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
        
        if name.len() >= METRIC_NAME_MAX_LEN as usize {
            return Err(format!("name longer than {} bytes", METRIC_NAME_MAX_LEN - 1));
        }
        if shorthelp_text.len() >= STRING_BLOCK_LEN as usize {
            return Err(format!("short help text longer than {} bytes", STRING_BLOCK_LEN - 1));
        }
        if longhelp_text.len() >= STRING_BLOCK_LEN as usize {
            return Err(format!("long help text longer than {} bytes", STRING_BLOCK_LEN - 1));
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
}

#[derive(Clone)]
/// An instance domain is a set of instances
pub struct Indom {
    pub (super) instances: HashSet<String>,
    pub (super) id: u32,
    shorthelp: String,
    longhelp: String
}

impl Indom {
    /// Creates a new instance domain with given instances, and short and long help text
    pub fn new(instances: &[&str], shorthelp_text: &str, longhelp_text: &str) -> Result<Self, String> {
        let mut hasher = DefaultHasher::new();
        instances.hash(&mut hasher);

        for instance in instances {
            if instance.len() >= METRIC_NAME_MAX_LEN as usize {
                return Err(format!("instance longer than {} bytes", METRIC_NAME_MAX_LEN - 1));
            }
        }
        if shorthelp_text.len() >= STRING_BLOCK_LEN as usize {
            return Err(format!("short help text longer than {} bytes", STRING_BLOCK_LEN - 1));
        }
        if longhelp_text.len() >= STRING_BLOCK_LEN as usize {
            return Err(format!("long help text longer than {} bytes", STRING_BLOCK_LEN - 1));
        }

        Ok(Indom {
            instances: instances.into_iter().map(|inst| inst.to_string()).collect(),
            id: (hasher.finish() as u32) & ((1 << INDOM_BIT_LEN) - 1),
            shorthelp: shorthelp_text.to_owned(),
            longhelp: longhelp_text.to_owned()
        })
    }

    /// Returns the number of instances in the domain
    pub fn instance_count(&self) -> u32 {
        self.instances.len() as u32
    }

    /// Checks if given instance is in the domain
    pub fn has_instance(&self, instance: &str) -> bool {
        self.instances.contains(instance)
    }

    pub fn shorthelp(&self) -> &str { &self.shorthelp }
    pub fn longhelp(&self) -> &str { &self.longhelp }

    pub (super) fn instance_id(instance: &str) -> u32 {
        let mut hasher = DefaultHasher::new();
        instance.hash(&mut hasher);
        hasher.finish() as u32
    }
}

pub (super) struct Instance<T> {
    pub (super) val: T,
    pub (super) mmap_view: MmapViewSync
}

/// An instance metric is a set of related metrics with same
/// type, semantics and unit. Many instance metrics can share
/// the same set of instances, i.e., instance domain.
pub struct InstanceMetric<T> {
    pub (super) indom: Indom,
    pub (super) vals: HashMap<String, Instance<T>>,
    pub (super) metric: Metric<T>
}

impl<T: MetricType + Clone> InstanceMetric<T> {
    /// Creates an instance metric with given name, initial value,
    /// semantics, unit, and short and long help text
    pub fn new(
        indom: &Indom,
        name: &str,
        init_val: T,
        sem: Semantics,
        unit: Unit,
        shorthelp_text: &str,
        longhelp_text: &str) -> Result<Self, String> {

        let mut vals = HashMap::with_capacity(indom.instances.len());
        let mut metric_name = name.to_owned();
        metric_name.push('.');
        for instance_str in &indom.instances {
            metric_name.push_str(instance_str);

            let instance = Instance {
                val: init_val.clone(),
                mmap_view: unsafe { SCRATCH_VIEW.clone() }
            };
            vals.insert(instance_str.to_owned(), instance);

            metric_name.truncate(name.len() + 1);
        }

        let mut metric = Metric::new(name, init_val.clone(), sem, unit, shorthelp_text, longhelp_text)?;
        metric.indom = indom.id;
        
        Ok(InstanceMetric {
            indom: indom.clone(),
            vals: vals,
            metric: metric
        })
    }

    /// Returns the number of instances that're part of the metric
    pub fn instance_count(&self) -> u32 {
        self.vals.len() as u32
    }

    /// Check if given instance is part of the metric
    pub fn has_instance(&self, instance: &str) -> bool {
        self.vals.contains_key(instance)
    }

    /// Returns the value of the given instance
    pub fn val(&self, instance: &str) -> Option<T> {
        self.vals.get(instance).map(|i| i.val.clone())
    }

    /// Sets the value of the given instance. If the instance isn't
    /// found, returns `None`.
    pub fn set_val(&mut self, instance: &str, new_val: T) -> Option<io::Result<()>>  {
        self.vals.get_mut(instance).map(|i| {
            new_val.write_to_writer(unsafe { &mut i.mmap_view.as_mut_slice() })?;
            i.val = new_val;
            Ok(())
        })
    }

    pub fn name(&self) -> &str { &self.metric.name }
    pub fn sem(&self) -> &Semantics { &self.metric.sem }
    pub fn unit(&self) -> u32 { self.metric.unit }
    pub fn shorthelp(&self) -> &str { &self.metric.shorthelp }
    pub fn longhelp(&self) -> &str { &self.metric.longhelp }
}

#[test]
fn test_instance_metrics() {
    use super::Client;

    let caches = Indom::new(
        &["L1", "L2", "L3"],
        "Caches",
        "Different levels of CPU caches"
    ).unwrap();

    let mut cache_sizes = InstanceMetric::new(
        &caches,
        "cache_size",
        0,
        Semantics::Discrete,
        Unit::new().space(Space::KByte, 1).unwrap(),
        "Cache sizes",
        "Sizes of different CPU caches"
    ).unwrap();

    assert!(cache_sizes.has_instance("L1"));
    assert!(!cache_sizes.has_instance("L4"));

    assert_eq!(cache_sizes.val("L2").unwrap(), 0);
    assert!(cache_sizes.val("L5").is_none());

    let mut cpu = Metric::new(
        "cpu",
        String::from("kabylake"),
        Semantics::Discrete,
        Unit::new(),
        "CPU family", "",
    ).unwrap();

    Client::new("system").unwrap()
        .begin_all(1, 3, 1, 1).unwrap()
        .register_instance_metric(&mut cache_sizes).unwrap()
        .register_metric(&mut cpu).unwrap()
        .export().unwrap();

    assert!(cache_sizes.set_val("L3", 8192).is_some());
    assert_eq!(cache_sizes.val("L3").unwrap(), 8192);
    
    assert!(cache_sizes.set_val("L4", 16384).is_none());
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
        .take(METRIC_NAME_MAX_LEN as usize).collect();
    let m1 = Metric::new(
        &invalid_name,
        0, Semantics::Discrete, Unit::new(), "", "",
    );
    assert!(m1.is_err());

    let invalid_shorthelp: String = thread_rng().gen_ascii_chars()
        .take(STRING_BLOCK_LEN as usize).collect();
    let m2 = Metric::new(
        "", 0, Semantics::Discrete, Unit::new(),
        &invalid_shorthelp,
        "",
    );
    assert!(m2.is_err());

    let invalid_longhelp: String = thread_rng().gen_ascii_chars()
        .take(STRING_BLOCK_LEN as usize).collect();
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
    client.begin_metrics(n_metrics as u64).unwrap();
    
    for _ in 1..n_metrics {
        let rnd_name: String = thread_rng().gen_ascii_chars()
            .take(METRIC_NAME_MAX_LEN as usize - 1).collect();

        let rnd_shorthelp: String = thread_rng().gen_ascii_chars()
            .take(STRING_BLOCK_LEN as usize - 1).collect();

        let rnd_longhelp: String = thread_rng().gen_ascii_chars()
            .take(STRING_BLOCK_LEN as usize - 1).collect();

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
        .begin_metrics(3).unwrap()
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
