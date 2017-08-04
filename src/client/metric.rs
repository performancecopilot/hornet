use byteorder::WriteBytesExt;
use memmap::{Mmap, MmapViewSync, Protection};
use std::collections::HashSet;
use std::collections::hash_map::{DefaultHasher, HashMap};
use std::fmt;
use std::hash::{Hash, Hasher};
use std::io;
use std::io::{Write, Cursor};
use std::mem;
use std::str;

use super::super::mmv::MTCode;
use super::super::{
    Endian,
    ITEM_BIT_LEN,
    INDOM_BIT_LEN,
    METRIC_NAME_MAX_LEN,
    STRING_BLOCK_LEN,
    METRIC_BLOCK_LEN,
    VALUE_BLOCK_LEN,
    NUMERIC_VALUE_SIZE,
    INDOM_BLOCK_LEN,
    INSTANCE_BLOCK_LEN
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
        fn write<W: WriteBytesExt>(&self, writer: &mut W) -> io::Result<()>;
    }

    use memmap::MmapViewSync;
    use std::collections::HashMap;
    
    pub struct MMVWriterState {
        // Mmap view of the entier MMV file
        pub mmap_view: Option<MmapViewSync>,

        // generation numbers
        pub gen: i64,
        pub gen2_off: u64,

        // counts
        pub n_toc: u64,
        pub n_metrics: u64,
        pub n_values: u64,
        pub n_strings: u64,
        pub n_indoms: u64,
        pub n_instances: u64,

        // caches
        pub non_value_string_cache: HashMap<String, Option<u64>>, // (string, offset to it)
        // if the offset is None, it means the string hasn't been written yet
        //
        pub indom_cache: HashMap<u32, Option<Vec<u64>>>, // (indom_id, offsets to it's instances)
        // if the offsets vector is None, it means the instances haven't been written yet

        // offsets to blocks
        pub indom_sec_off: u64,
        pub instance_sec_off: u64,
        pub metric_sec_off: u64,
        pub value_sec_off: u64,
        pub string_sec_off: u64,
        pub string_toc_off: u64,

        // running indexes of objects written so far
        pub indom_idx: u64,
        pub instance_idx: u64,
        pub metric_blk_idx: u64,
        pub value_blk_idx: u64,
        pub string_blk_idx: u64,

        // mmv header data
        pub flags: u32,
        pub cluster_id: u32
    }

    impl MMVWriterState {
        pub fn new() -> Self {
            MMVWriterState {
                mmap_view: None,

                gen: 0,
                gen2_off: 0,

                n_toc: 0,
                n_metrics: 0,
                n_values: 0,
                n_strings: 0,
                n_indoms: 0,
                n_instances: 0,

                indom_cache: HashMap::new(),
                non_value_string_cache: HashMap::new(),

                indom_sec_off: 0,
                instance_sec_off: 0,
                metric_sec_off: 0,
                value_sec_off: 0,
                string_sec_off: 0,
                string_toc_off: 0,

                indom_idx: 0,
                instance_idx: 0,
                metric_blk_idx: 0,
                value_blk_idx: 0,
                string_blk_idx: 0,

                flags: 0,
                cluster_id: 0
            }
        }
    }

    /// MMV object that writes blocks to an MMV
    pub trait MMVWriter {
        private_decl!{}

        fn write(&mut self,
            writer_state: &mut MMVWriterState,
            cursor: &mut io::Cursor<&mut [u8]>) -> io::Result<()>;

        fn register(&self, ws: &mut MMVWriterState);
    }
}

pub (super) use self::private::MetricType;
pub (super) use self::private::{MMVWriter, MMVWriterState};

macro_rules! impl_metric_type_for (
    ($typ:tt, $base_typ:tt, $type_code:expr) => (
        impl MetricType for $typ {

            private_impl!{}

            fn type_code(&self) -> u32 {
                $type_code as u32
            }

            fn write<W: WriteBytesExt>(&self, w: &mut W)
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

    fn write<W: Write>(&self, writer: &mut W) -> io::Result<()> {
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
            if $dim.abs() > 1 {
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
            write!(f, "/ ")?;
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
    val: T,
    mmap_view: MmapViewSync
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
        new_val.write(unsafe { &mut self.mmap_view.as_mut_slice() })?;
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
    instances: HashSet<String>,
    id: u32,
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

    fn instance_id(instance: &str) -> u32 {
        let mut hasher = DefaultHasher::new();
        instance.hash(&mut hasher);
        hasher.finish() as u32
    }
}

struct Instance<T> {
    val: T,
    mmap_view: MmapViewSync
}

/// An instance metric is a set of related metrics with same
/// type, semantics and unit. Many instance metrics can share
/// the same set of instances, i.e., instance domain.
pub struct InstanceMetric<T> {
    indom: Indom,
    vals: HashMap<String, Instance<T>>,
    metric: Metric<T>
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
            new_val.write(unsafe { &mut i.mmap_view.as_mut_slice() })?;
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

impl<T: MetricType> Metric<T> {
    fn write_to_mmv(&mut self, ws: &mut MMVWriterState, c: &mut Cursor<&mut [u8]>,
                write_value_blk: bool) -> io::Result<u64> {

        let orig_pos = c.position();

        // metric block
        let metric_blk_off =
            ws.metric_sec_off
            + ws.metric_blk_idx*METRIC_BLOCK_LEN;
        c.set_position(metric_blk_off);
        // name
        c.write_all(self.name.as_bytes())?;
        c.write_all(&[0])?;
        c.set_position(metric_blk_off + METRIC_NAME_MAX_LEN);
        // item
        c.write_u32::<Endian>(self.item)?;
        // type code
        c.write_u32::<Endian>(self.val.type_code())?;
        // sem
        c.write_u32::<Endian>(self.sem as u32)?;
        // unit
        c.write_u32::<Endian>(self.unit)?;
        // indom
        c.write_u32::<Endian>(self.indom)?;
        // zero pad
        c.write_u32::<Endian>(0)?;
        // short help
        if self.shorthelp.len() > 0 {
            let short_help_off = write_mmv_string(ws, c, &self.shorthelp, false)?;
            c.write_u64::<Endian>(short_help_off)?;
        }
        // long help
        if self.longhelp.len() > 0 {
            let long_help_off = write_mmv_string(ws, c, &self.longhelp, false)?;
            c.write_u64::<Endian>(long_help_off)?;
        }

        if write_value_blk {
            let (value_offset, value_size) =
                write_value_block(ws, c, &self.val, metric_blk_off, 0)?;

            let mmap_view = unsafe {
                ws.mmap_view.as_mut().unwrap().clone()
            };
            let (_, value_mmap_view, _) =
                three_way_split(mmap_view, value_offset, value_size)?;
            self.mmap_view = value_mmap_view;
        }

        ws.metric_blk_idx += 1;
        c.set_position(orig_pos);
        Ok(metric_blk_off)
    }
}

impl<T: MetricType> MMVWriter for Metric<T> {
    private_impl!{}

    fn write(&mut self, ws: &mut MMVWriterState, c: &mut Cursor<&mut [u8]>) -> io::Result<()> {
        self.write_to_mmv(ws, c, true)?;
        Ok(())
    }

    fn register(&self, ws: &mut MMVWriterState) {
        ws.n_metrics += 1;
        ws.n_values += 1;

        if self.val.type_code() == MTCode::String as u32 {
            ws.n_strings += 1;
        }

        cache_and_register_string(ws, &self.shorthelp);
        cache_and_register_string(ws, &self.longhelp);
    }
}

impl<T: MetricType> MMVWriter for InstanceMetric<T> {
    private_impl!{}

    fn write(&mut self, ws: &mut MMVWriterState, c: &mut Cursor<&mut [u8]>) -> io::Result<()> {
        // write metric block
        let metric_blk_off = self.metric.write_to_mmv(ws, c, false)?;

        // write indom and instances
        let instance_blk_offs = write_indom_and_instances(ws, c, &self.indom)?;

        // write value blocks
        for ((_, instance), instance_blk_off) in self.vals.iter_mut().zip(instance_blk_offs) {

            let (value_offset, value_size) =
                write_value_block(ws, c, &instance.val, metric_blk_off, instance_blk_off)?;

            // set mmap_view for instance
            let mmap_view = unsafe {
                ws.mmap_view.as_mut().unwrap().clone()
            };
            let (_, value_mmap_view, _) =
                three_way_split(mmap_view, value_offset, value_size)?;
            instance.mmap_view = value_mmap_view;
        }

        Ok(())
    }

    fn register(&self, ws: &mut MMVWriterState) {
        ws.n_metrics += 1;
        ws.n_values += self.vals.len() as u64;

        if self.metric.val.type_code() == MTCode::String as u32 {
            ws.n_strings += 1;
        }

        cache_and_register_string(ws, &self.metric.shorthelp);
        cache_and_register_string(ws, &self.metric.longhelp);
        cache_and_register_string(ws, &self.indom.shorthelp);
        cache_and_register_string(ws, &self.indom.longhelp);

        if !ws.indom_cache.contains_key(&self.indom.id) {
            ws.n_indoms += 1;
            ws.n_instances += self.indom.instances.len() as u64;
            ws.indom_cache.insert(self.indom.id, None);
        }
    }
}

fn write_indom_and_instances<'a>(ws: &mut MMVWriterState, c: &mut Cursor<&mut [u8]>,
    indom: &Indom)-> io::Result<Vec<u64>> {

    // write each indom and it's instances only once
    if let Some(blk_offs) = ws.indom_cache.get(&indom.id) {
        if let &Some(ref blk_offs) = blk_offs {
            return Ok(blk_offs.clone())
        }
    }

    // write indom block
    let indom_off =
        ws.indom_sec_off
        + INDOM_BLOCK_LEN*ws.indom_idx;
    c.set_position(indom_off);
    // indom id
    c.write_u32::<Endian>(indom.id)?;
    // number of instances
    c.write_u32::<Endian>(indom.instance_count())?;
    // offset to instances
    let mut instance_blk_off =
        ws.instance_sec_off
        + INSTANCE_BLOCK_LEN*ws.instance_idx;
    c.write_u64::<Endian>(instance_blk_off)?;
    // short help
    if indom.shorthelp().len() > 0 {
        let short_help_off = 
            write_mmv_string(ws, c, indom.shorthelp(), false)?;
        c.write_u64::<Endian>(short_help_off)?;
    }
    // long help
    if indom.longhelp().len() > 0 {
        let long_help_off = 
            write_mmv_string(ws, c, indom.longhelp(), false)?;
        c.write_u64::<Endian>(long_help_off)?;
    }

    // write instances and record their offsets
    let mut instance_blk_offs = Vec::with_capacity(indom.instances.len());
    for instance in &indom.instances {
        c.set_position(instance_blk_off);

        // indom offset
        c.write_u64::<Endian>(indom_off)?;
        // zero pad
        c.write_u32::<Endian>(0)?;
        // instance id
        c.write_u32::<Endian>(Indom::instance_id(&instance))?;
        // instance
        c.write_all(instance.as_bytes())?;
        c.write_all(&[0])?;

        instance_blk_offs.push(instance_blk_off);
        instance_blk_off += INSTANCE_BLOCK_LEN;
    }

    ws.instance_idx += instance_blk_offs.len() as u64;
    ws.indom_idx += 1;

    let cloned_offs = instance_blk_offs.clone();
    ws.indom_cache.insert(indom.id, Some(instance_blk_offs));
    Ok(cloned_offs)
}

fn three_way_split(view: MmapViewSync, mid_idx: usize, mid_len: usize) -> io::Result<(MmapViewSync, MmapViewSync, MmapViewSync)> {
    let (left_view, mid_right_view) = view.split_at(mid_idx).unwrap();
    let (mid_view, right_view) = mid_right_view.split_at(mid_len).unwrap();
    Ok((left_view, mid_view, right_view))
}

// writes `val` at end of value section, updates value count in value TOC,
// and returns the offset `val` was written at and it's size - (offset, size)
//
// leaves the cursor in the original position it was at when passed
fn write_value_block<T: MetricType>(ws: &mut MMVWriterState,
    mut c: &mut Cursor<&mut [u8]>, value: &T,
    metric_blk_off: u64, instance_blk_off: u64) -> io::Result<(usize, usize)> {

    let orig_pos = c.position();

    let value_blk_off =
        ws.value_sec_off
        + ws.value_blk_idx*VALUE_BLOCK_LEN;
    ws.value_blk_idx += 1;
    c.set_position(value_blk_off);

    let (value_offset, value_size);
    if value.type_code() == MTCode::String as u32 {
        // numeric value
        c.write_u64::<Endian>(0)?;

        // string offset

        // we can't pass the actual `m.val()` string to write_mmv_string,
        // and in order to not replicate the logic of write_mmv_string here,
        // we perform an extra write of the string to a temp buffer so we
        // can pass that to write_mmv_string.
        let mut str_buf = [0u8; (STRING_BLOCK_LEN - 1) as usize];
        value.write(&mut (&mut str_buf as &mut [u8]))?;

        let str_val = unsafe { str::from_utf8_unchecked(&str_buf) };
        let string_val_off = write_mmv_string(ws, c, str_val, true)?;
        c.write_u64::<Endian>(string_val_off)?;

        value_offset = string_val_off as usize;
        value_size = STRING_BLOCK_LEN as usize;
    } else {
        value_offset = c.position() as usize;
        value_size = NUMERIC_VALUE_SIZE;

        // numeric value
        value.write(&mut c)?;
        // string offset
        c.write_u64::<Endian>(0)?;
    }
    // offset to metric block
    c.write_u64::<Endian>(metric_blk_off)?;
    // offset to instance block
    c.write_u64::<Endian>(instance_blk_off)?;
    
    c.set_position(orig_pos);
    Ok((value_offset, value_size))
}

fn cache_and_register_string(ws: &mut MMVWriterState, string: &str) {
    if string.len() > 0 && !ws.non_value_string_cache.contains_key(string) {
        ws.non_value_string_cache.insert(string.to_owned(), None);
        ws.n_strings += 1;
    }
}

// writes `string` at end of string section, updates string count in string TOC,
// and returns the offset `string` was written at
//
// leaves the cursor in the original position it was at when passed
//
// when writing first string in MMV, also writes the string TOC block
fn write_mmv_string(ws: &mut MMVWriterState,
    c: &mut Cursor<&mut [u8]>, string: &str, is_value: bool) -> io::Result<u64> {

    let orig_pos = c.position();

    let string_block_off =
        ws.string_sec_off
        + STRING_BLOCK_LEN*ws.string_blk_idx;
        
    // only cache if the string is not a value
    if !is_value {
        if let Some(cached_offset) = ws.non_value_string_cache.get(string).clone() {
            if let &Some(off) = cached_offset {
                return Ok(off);
            }
        }

        ws.non_value_string_cache.insert(string.to_owned(), Some(string_block_off));
    }

    // write string in string section
    c.set_position(string_block_off);
    c.write_all(string.as_bytes())?;
    c.write_all(&[0])?;

    ws.string_blk_idx += 1;

    c.set_position(orig_pos);
    Ok(string_block_off)
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
        .export(&mut [&mut cache_sizes, &mut cpu]).unwrap();

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

    let client = Client::new("numeric_metrics").unwrap();
    
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

        metrics.push(metric);
        new_vals.push(thread_rng().gen::<u32>());
    }

    { // mmv_writers needs to go out of scope before we can mutate
      // the metrics after exporting. The type annotation is needed
      // because type inference fails.
        let mut mmv_writers: Vec<&mut MMVWriter> = metrics.iter_mut()
            .map(|m| m as &mut MMVWriter)
            .collect();
        client.export(&mut mmv_writers).unwrap();
    }

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
        .export(&mut [&mut freq, &mut color, &mut photons]).unwrap();

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
