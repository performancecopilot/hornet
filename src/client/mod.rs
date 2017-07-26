use byteorder::WriteBytesExt;
use memmap::{Mmap, MmapViewSync, Protection};
use regex::bytes::Regex;
use std::collections::HashMap;
use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::fs::{File, OpenOptions};
use std::io;
use std::io::{BufReader, Cursor};
use std::io::prelude::*;
use std::path::{MAIN_SEPARATOR, Path, PathBuf};
use std::str;
use time;

use super::{
    Endian,
    CLUSTER_ID_BIT_LEN,
    HDR_LEN,
    TOC_BLOCK_LEN,
    METRIC_BLOCK_LEN,
    VALUE_BLOCK_LEN,
    STRING_BLOCK_LEN,
    NUMERIC_VALUE_SIZE,
    METRIC_NAME_MAX_LEN,
    MIN_STRINGS_PER_METRIC,
    INDOM_BLOCK_LEN,
    INSTANCE_BLOCK_LEN
};

pub mod metric;
use self::metric::{Indom, InstanceMetric, Metric, MetricType, MTCode};

static PCP_TMP_DIR_KEY: &'static str = "PCP_TMP_DIR";
static MMV_DIR_SUFFIX: &'static str = "mmv";

#[cfg(unix)]
fn get_process_id() -> i32 {
    use nix;
    nix::unistd::getpid()
}

#[cfg(windows)]
fn get_process_id() -> i32 {
    use kernel32;
    unsafe { kernel32::GetCurrentProcessId() as i32 }
}

#[cfg(unix)]
fn osstr_from_bytes(slice: &[u8]) -> &OsStr {
    use std::os::unix::ffi::OsStrExt;
    OsStr::from_bytes(slice)
}

#[cfg(windows)]
fn osstr_from_bytes(slice: &[u8]) -> &OsStr {
    OsStr::new(unsafe { str::from_utf8_unchecked(slice) })
}

fn get_pcp_root() -> PathBuf {
    match env::var_os("PCP_DIR") {
        Some(val) => PathBuf::from(val),
        None => PathBuf::from(MAIN_SEPARATOR.to_string())
    }
}

fn init_pcp_conf(pcp_root: &Path) -> io::Result<()> {
    /* attempt to load variables from pcp_root/etc/pcp.conf into environment.
       if pcp_root/etc/pcp.conf is not a file, can't be read, or parsing it
       fails, we *don't* return the error */
    parse_pcp_conf(pcp_root.join("etc").join("pcp.conf")).ok();

    /* attempt to load variables from pcp_root/$PCP_CONF into environment.
       if pcp_root/$PCP_CONF is not a file, can't be read, or parsing it
       fails, we *do* return the error */
    let pcp_conf = pcp_root
        .join(env::var_os("PCP_CONF").unwrap_or(OsString::new()));
    parse_pcp_conf(pcp_conf)
}

fn parse_pcp_conf<P: AsRef<Path>>(conf_path: P) -> io::Result<()> {
    let pcp_conf = File::open(conf_path)?;
    let mut buf_reader = BufReader::new(pcp_conf);

    /* According to man 5 pcp.conf, syntax rules for pcp.conf are
        1. general syntax is PCP_VARIABLE_NAME=value to end of line 
        2. blank lines and lines begining with # are ignored
        3. variable names that aren't prefixed with PCP_ are silently ignored
        4. there should be no space between the variable name and the literal =
        5. values may contain spaces and should not be quoted
    */
    lazy_static! {
        static ref RE: Regex =
            Regex::new("(?-u)^(PCP_[[:alnum:]_]+)=([^\"\'].*[^\"\'])\n$")
                .unwrap();
    }

    let mut line = Vec::new();
    while buf_reader.read_until(b'\n', &mut line)? > 0 {
        match RE.captures(&line) {
            Some(caps) => {
                match (caps.get(1), caps.get(2)) {
                    (Some(key), Some(val)) => env::set_var(
                        osstr_from_bytes(key.as_bytes()), 
                        osstr_from_bytes(val.as_bytes()), 
                    ),
                    _ => {}
                }
            }
            _ => {}
        }
        line.clear();
    }

    Ok(())
}

fn get_mmv_dir() -> io::Result<PathBuf> {
    let pcp_root = get_pcp_root();
    let mut mmv_dir = pcp_root.clone();

    mmv_dir.push(match env::var_os(PCP_TMP_DIR_KEY) {
        Some(val) => PathBuf::from(val),
        None => {

            init_pcp_conf(&pcp_root).ok();

            /* re-check if PCP_TMP_DIR is set after parsing (any) conf files
               if not, default to OS-specific temp dir and set PCP_TMP_DIR
               so we don't enter this block again */
            match env::var_os(PCP_TMP_DIR_KEY) {
                Some(val) => PathBuf::from(val),
                None => {
                    let os_tmp_dir = env::temp_dir();
                    env::set_var(PCP_TMP_DIR_KEY, os_tmp_dir.as_os_str());
                    os_tmp_dir
                }
            }
        }
    });

    mmv_dir.push(MMV_DIR_SUFFIX);
    fs::create_dir_all(&mmv_dir)?;

    Ok(mmv_dir)
}

bitflags! {
    /// Flags used to modify how a client exports metrics
    pub struct MMVFlags: u32 {
        /// Metric names aren't prefixed with MMV filename
        const NOPREFIX = 1;
        /// PID check is needed
        const PROCESS  = 2;
        /// Allow "no value available" values
        const SENTINEL = 4;
    }
}

struct MMVWriterInfo {
    mmap_view: Option<MmapViewSync>,

    // generation number
    gen: i64,
    gen2off: u64,

    // counts of various *things*
    n_indoms: u64,
    n_instances: u64,
    n_instance_metrics: u64,
    n_metrics: u64,
    n_metric_blks: u64, // n_metrics + n_instance_metrics
    n_value_blks: u64,
    n_strings: u64,
    n_toc: u64,

    // offsets to various blocks
    indom_sec_off: u64,
    instance_sec_off: u64,
    metric_sec_off: u64,
    value_sec_off: u64,
    string_sec_off: u64,
    string_toc_off: u64,

    // offsets to counts
    n_toc_off: u64,
    n_strings_off: u64,

    // running indexes of registered *things*
    indom_idx: u64,
    instance_idx: u64,
    metric_blk_idx: u64,
    value_blk_idx: u64,

    // caches
    indom_cache: HashMap<u32, Vec<u64>>, // (indom_id, offsets to it's instances)
    string_cache: HashMap<String, u64> // (string, offset to it)
}

impl MMVWriterInfo {
    fn new() -> Self {
        MMVWriterInfo {
            mmap_view: None,
            n_indoms: 0,
            n_instances: 0,
            n_instance_metrics: 0,
            n_metrics: 0,
            n_metric_blks: 0,
            n_value_blks: 0,
            n_strings: 0,
            gen: 0,
            gen2off: 0,
            indom_sec_off: 0,
            instance_sec_off: 0,
            metric_sec_off: 0,
            value_sec_off: 0,
            value_blk_idx: 0,
            string_sec_off: 0,
            metric_blk_idx: 0,
            instance_idx: 0,
            indom_idx: 0,
            n_strings_off: 0,
            string_toc_off: 0,
            n_toc: 0,
            n_toc_off: 0,
            indom_cache: HashMap::new(),
            string_cache: HashMap::new()
        }
    }
}

/// Client used to export metrics
pub struct Client {
    flags: MMVFlags,
    cluster_id: u32,
    mmv_path: PathBuf,
    wi: MMVWriterInfo
}

impl Client {
    /// Creates a new client with `PROCESS` flag and `0` cluster ID
    pub fn new(name: &str) -> io::Result<Client> {
        Client::new_custom(name, PROCESS, 0)
    }

    /// Creates a new client with custom flags and cluster ID
    ///
    /// Note that only the 12 least significant bits of `cluster_id` will be
    /// used.
    pub fn new_custom(name: &str, flags: MMVFlags, cluster_id: u32)
    -> io::Result<Client> {
        let mmv_path = get_mmv_dir()?.join(name);
        let cluster_id = cluster_id & ((1 << CLUSTER_ID_BIT_LEN) - 1);

        Ok(Client {
            flags: flags,
            cluster_id: cluster_id,
            mmv_path: mmv_path,
            wi: MMVWriterInfo::new()
        })
    }

    /// Begins registration of singleton metrics only
    pub fn begin_metrics(&mut self, n_metrics: u64) -> io::Result<&mut Client> {
        self.begin_all(0, 0, 0, n_metrics)
    }

    /// Begins registration of singleton and instance metrics
    /// - `n_indoms` is the number of unique indoms
    /// - `n_instances` is the number of unique instances
    pub fn begin_all(&mut self, n_indoms: u64, n_instances: u64, n_instance_metrics: u64, n_metrics: u64) -> io::Result<&mut Client> {
        self.wi.n_metrics = n_metrics;
        self.wi.n_toc = 2; // metric + value

        if n_indoms > 0 && n_instances > 0 && n_instance_metrics > 0 {
            self.wi.n_indoms = n_indoms;
            self.wi.n_instances = n_instances;
            self.wi.n_instance_metrics = n_instance_metrics;
            self.wi.n_toc += 2; // indom + instance
        }
        self.wi.n_metric_blks = self.wi.n_metrics + self.wi.n_instance_metrics;
        self.wi.n_value_blks =
             self.wi.n_metrics +
             self.wi.n_instances * self.wi.n_instance_metrics; // FIXME: very wasteful allocation in some cases

        let hdr_toc_len =
            HDR_LEN +
            TOC_BLOCK_LEN*(self.wi.n_toc + 1 /* reserve space for possible string TOC */ );
        let mmv_size = (
            hdr_toc_len +
            self.wi.n_indoms*INDOM_BLOCK_LEN +
            self.wi.n_instances*INSTANCE_BLOCK_LEN +
            self.wi.n_metric_blks*(
                METRIC_BLOCK_LEN +
                MIN_STRINGS_PER_METRIC*STRING_BLOCK_LEN
            ) +
            self.wi.n_value_blks*(
                VALUE_BLOCK_LEN +
                STRING_BLOCK_LEN
            )
        ) as usize;
        
        let mut file = OpenOptions::new().read(true).write(true).create(true)
            .open(&self.mmv_path)?;
        file.write(&vec![0; mmv_size])?;

        let mut mmap_view = Mmap::open(&file, Protection::ReadWrite)?
            .into_view_sync();

        /*
            The layout of the MMV is as follows:

            --- MMV Header
            
            --- Instance Domain TOC Block

            --- Instances TOC Block

            --- Metrics TOC Block
            
            --- Values TOC Block

            --- Strings TOC Block

            --- Instance Domain section
                --- instance domain block 1
                --- instance domain block 2
                --- ...

            --- Instances section
                --- instance block 1 for instance domain 1
                --- instance block 2 for instance domain 1
                --- instance block 1 for instance domain 2
                --- ...

            --- Metrics section
                --- metric block 1
                --- metric block 2
                --- ...

            --- Values section
                --- value block for metric 1
                --- value block for metric 2
                --- ...

            --- Strings section
                --- (optional) short help text for metric 1
                --- (optional) long help text  for metric 1
                --- (optional) string value    for metric 1 
                --- (optional) short help text for metric 2
                --- ...
            
            After writing, every metric is given ownership
            of the respective memory-mapped slice that contains
            the metric's value. This is to ensure that the metric
            is *only* able to write to it's value slice when updating
            it's value.
        */
 
        {
            let mut cur = Cursor::new(unsafe { mmap_view.as_mut_slice() });
            self.write_mmv_header(&mut cur)?;

            if self.wi.n_indoms > 0 {
                self.wi.indom_sec_off = hdr_toc_len;
                self.wi.instance_sec_off = self.wi.indom_sec_off + INDOM_BLOCK_LEN*self.wi.n_indoms;
                self.wi.metric_sec_off = self.wi.instance_sec_off + INSTANCE_BLOCK_LEN*self.wi.n_instances;
 
                self.write_indom_toc_block(&mut cur)?;
                self.write_instance_toc_block(&mut cur)?;
            } else {
                self.wi.metric_sec_off = hdr_toc_len;
            }
            self.write_metric_toc_block(&mut cur)?;

            self.wi.value_sec_off = self.wi.metric_sec_off + METRIC_BLOCK_LEN*self.wi.n_metric_blks;
            self.write_values_toc_block(&mut cur)?;

            self.wi.string_sec_off = self.wi.value_sec_off + VALUE_BLOCK_LEN*self.wi.n_value_blks;
            self.wi.string_toc_off = cur.position();
        }

        self.wi.mmap_view = Some(mmap_view);
        Ok(self)
    }

    fn write_mmv_header(&mut self, c: &mut Cursor<&mut [u8]>) -> io::Result<()> {    
        // MMV\0
        c.write_all(b"MMV\0")?;
        // version
        c.write_u32::<Endian>(1)?;
        // generation1
        self.wi.gen = time::now().to_timespec().sec;
        c.write_i64::<Endian>(self.wi.gen)?;
        // generation2
        self.wi.gen2off = c.position();
        c.write_i64::<Endian>(0)?;
        // no. of toc blocks
        self.wi.n_toc_off = c.position();
        c.write_u32::<Endian>(self.wi.n_toc as u32)?;
        // flags
        c.write_u32::<Endian>(self.flags.bits())?;
        // pid
        c.write_i32::<Endian>(get_process_id())?;
        // cluster id
        c.write_u32::<Endian>(self.cluster_id)
    }

    fn write_indom_toc_block(&mut self, c: &mut Cursor<&mut [u8]>) -> io::Result<()> {
        // section type
        c.write_u32::<Endian>(1)?;
        // no. of entries
        c.write_u32::<Endian>(self.wi.n_indoms as u32)?;
        // section offset
        c.write_u64::<Endian>(self.wi.indom_sec_off)
    }

    fn write_instance_toc_block(&mut self, c: &mut Cursor<&mut [u8]>) -> io::Result<()> {
        // section type
        c.write_u32::<Endian>(2)?;
        // no. of entries
        c.write_u32::<Endian>(self.wi.n_instances as u32)?;
        // section offset
        c.write_u64::<Endian>(self.wi.instance_sec_off)
    }
    
    fn write_metric_toc_block(&mut self, c: &mut Cursor<&mut [u8]>) -> io::Result<()> {
        // section type
        c.write_u32::<Endian>(3)?;
        // no. of entries
        c.write_u32::<Endian>(self.wi.n_metric_blks as u32)?;
        // section offset
        c.write_u64::<Endian>(self.wi.metric_sec_off)
    }

    fn write_values_toc_block(&mut self, c: &mut Cursor<&mut [u8]>) -> io::Result<()> {
        // section type
        c.write_u32::<Endian>(4)?;
        // no. of entries
        c.write_u32::<Endian>(self.wi.n_value_blks as u32)?;
        // section offset
        c.write_u64::<Endian>(self.wi.value_sec_off)
    }

    pub fn register_metric<T: MetricType + Clone, M: AsMut<Metric<T>>>
        (&mut self, mut metric: M) -> io::Result<&mut Client> {

        let mut mmap_view = unsafe { self.wi.mmap_view.as_mut().unwrap().clone() };
        let mut c = Cursor::new(unsafe { mmap_view.as_mut_slice() });
        self.register_metric_common(&mut c, metric.as_mut(), true)?;
        Ok(self)
    }

    pub fn register_instance_metric<T: MetricType + Clone>(&mut self, im: &mut InstanceMetric<T>) -> io::Result<&mut Client> {
        let mut mmap_view = unsafe { self.wi.mmap_view.as_mut().unwrap().clone() };
        let mut c = Cursor::new(unsafe { mmap_view.as_mut_slice() });

        // write each indom and it's instances only once
        let indom = &im.indom;
        if !self.wi.indom_cache.contains_key(&indom.id) {

            // write indom block
            let indom_off = self.wi.indom_sec_off + INDOM_BLOCK_LEN*self.wi.indom_idx;
            c.set_position(indom_off);
            // indom id
            c.write_u32::<Endian>(indom.id)?;
            // number of instances
            c.write_u32::<Endian>(indom.instance_count())?;
            // offset to instances
            let mut instance_blk_off = self.wi.instance_sec_off + INSTANCE_BLOCK_LEN*self.wi.instance_idx;
            c.write_u64::<Endian>(instance_blk_off)?;
            // short help
            if indom.shorthelp().len() > 0 {
                let short_help_off = self.write_mmv_string(&mut c, indom.shorthelp(), false)?;
                c.write_u64::<Endian>(short_help_off)?;
            }
            // long help
            if indom.longhelp().len() > 0 {
                let long_help_off = self.write_mmv_string(&mut c, indom.longhelp(), false)?;
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

            self.wi.instance_idx += im.vals.len() as u64;
            self.wi.indom_idx += 1;

            self.wi.indom_cache.insert(indom.id, instance_blk_offs);
        }

        // write metric block
        let metric_blk_off = self.register_metric_common(&mut c, &mut im.metric, false)?;

        // write value blocks
        let instance_blk_offs = self.wi.indom_cache.get(&indom.id).unwrap().clone();
        for ((_, instance), instance_blk_off) in im.vals.iter_mut().zip(instance_blk_offs) {

            let (value_offset, value_size) =
                self.write_value_block(&mut c, &instance.val, metric_blk_off, instance_blk_off)?;

            // set mmap_view for instance
            let mmap_view = unsafe {
                self.wi.mmap_view.as_mut().unwrap().clone()
            };
            let (_, value_mmap_view, _) =
                three_way_split(mmap_view, value_offset, value_size)?;
            instance.mmap_view = value_mmap_view;
        }

        Ok(self)
    }

    // writes `m` at end of metric section, and returns the offset `m` was written
    //
    // if the `m` is part of an instance metric, pass `write_val` as false so that
    // an extra value block isn't written
    //
    // leaves the cursor in the original position it was at when passed
    pub fn register_metric_common<T: MetricType + Clone>(&mut self, mut c: &mut Cursor<&mut [u8]>,
        m: &mut Metric<T>, write_val: bool) -> io::Result<u64> {

        let orig_pos = c.position();

        // TODO: return custom error instead of panicing
        assert!(self.wi.metric_blk_idx < self.wi.n_metric_blks);

        let i = self.wi.metric_blk_idx;

        // metric block
        let metric_blk_off = self.wi.metric_sec_off + i*METRIC_BLOCK_LEN;
        c.set_position(metric_blk_off);
        // name
        c.write_all(m.name().as_bytes())?;
        c.write_all(&[0])?;
        c.set_position(metric_blk_off + METRIC_NAME_MAX_LEN);
        // item
        c.write_u32::<Endian>(m.item())?;
        // type code
        let type_code = m.type_code();
        c.write_u32::<Endian>(type_code)?;
        // sem
        c.write_u32::<Endian>(*(m.sem()) as u32)?;
        // unit
        c.write_u32::<Endian>(m.unit())?;
        // indom
        c.write_u32::<Endian>(m.indom())?;
        // zero pad
        c.write_u32::<Endian>(0)?;
        // short help
        if m.shorthelp().len() > 0 {
            let short_help_off = self.write_mmv_string(&mut c, m.shorthelp(), false)?;
            c.write_u64::<Endian>(short_help_off)?;
        }
        // long help
        if m.longhelp().len() > 0 {
            let long_help_off = self.write_mmv_string(&mut c, m.longhelp(), false)?;
            c.write_u64::<Endian>(long_help_off)?;
        }

        // write value block
        if write_val {
            let (value_offset, value_size) =
                self.write_value_block(&mut c, &m.val, metric_blk_off, 0)?;

            // set mmap_view for metric
            let mmap_view = unsafe {
                self.wi.mmap_view.as_mut().unwrap().clone()
            };
            let (_, value_mmap_view, _) =
                three_way_split(mmap_view, value_offset, value_size)?;
            m.mmap_view = value_mmap_view;
        }

        self.wi.metric_blk_idx += 1;

        c.set_position(orig_pos);
        Ok(metric_blk_off)
    }

    // writes `val` at end of value section, updates value count in value TOC,
    // and returns the offset `val` was written at and it's size - (offset, size)
    //
    // leaves the cursor in the original position it was at when passed
    fn write_value_block<T: MetricType + Clone>(&mut self, mut c: &mut Cursor<&mut [u8]>, val: &T,
        metric_blk_off: u64, instance_blk_off: u64) -> io::Result<(usize, usize)> {

        let orig_pos = c.position();

        let value_blk_off = self.wi.value_sec_off + self.wi.value_blk_idx*VALUE_BLOCK_LEN;
        self.wi.value_blk_idx += 1;
        c.set_position(value_blk_off);

        let (value_offset, value_size);
        if val.type_code() == MTCode::String as u32 {
            // numeric value
            c.write_u64::<Endian>(0)?;

            // string offset

            // we can't pass the actual `m.val()` string to write_mmv_string,
            // and in order to not replicate the logic of write_mmv_string here,
            // we perform an extra write of the string to a temp buffer so we
            // can pass that to write_mmv_string.
            let mut str_buf = [0u8; STRING_BLOCK_LEN as usize];
            val.write_to_writer(&mut (&mut str_buf as &mut [u8]))?;

            let str_val = unsafe { str::from_utf8_unchecked(&str_buf) };
            let string_val_off = self.write_mmv_string(&mut c, str_val, true)?;
            c.write_u64::<Endian>(string_val_off)?;

            value_offset = string_val_off as usize;
            value_size = STRING_BLOCK_LEN as usize;
        } else {
            value_offset = c.position() as usize;
            value_size = NUMERIC_VALUE_SIZE;

            // numeric value
            val.write_to_writer(&mut c)?;
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

    // writes `string` at end of string section, updates string count in string TOC,
    // and returns the offset `string` was written at
    //
    // leaves the cursor in the original position it was at when passed
    //
    // when writing first string in MMV, also writes the string TOC block
    fn write_mmv_string(&mut self, c: &mut Cursor<&mut [u8]>, string: &str, is_value: bool) -> io::Result<u64> {
        let orig_pos = c.position();

        if self.wi.n_strings == 0 {
            // update toc count
            self.wi.n_toc += 1;
            c.set_position(self.wi.n_toc_off);
            c.write_u32::<Endian>(self.wi.n_toc as u32)?;

            // write string toc
            c.set_position(self.wi.string_toc_off);
            // section type
            c.write_u32::<Endian>(5)?;
            // no. of entries
            self.wi.n_strings_off = c.position();
            c.write_u32::<Endian>(self.wi.n_strings as u32)?;
            // section offset
            c.write_u64::<Endian>(self.wi.string_sec_off)?;
        }

        let string_block_off = self.wi.string_sec_off + STRING_BLOCK_LEN*self.wi.n_strings;

        // only use cache if the string is not a value string
        if !is_value {
            if let Some(cached_offset) = self.wi.string_cache.get(string).clone() {
                return Ok(*cached_offset);
            }
            self.wi.string_cache.insert(string.to_owned(), string_block_off);
        }

        // write string in string section
        c.set_position(string_block_off);
        c.write_all(string.as_bytes())?;
        c.write_all(&[0])?;

        // update string count in string toc
        self.wi.n_strings += 1;
        c.set_position(self.wi.n_strings_off);
        c.write_u32::<Endian>(self.wi.n_strings as u32)?;

        c.set_position(orig_pos);
        Ok(string_block_off)
    }

    pub fn export(&mut self) -> io::Result<()> {

        { // this block makes sure mmap_view goes out of scope before we return Ok(self)

            let mmap_view = self.wi.mmap_view.as_mut().unwrap();
            let mut cur = Cursor::new(unsafe { mmap_view.as_mut_slice() });

            // unlock MMV header
            cur.set_position(self.wi.gen2off);
            cur.write_i64::<Endian>(self.wi.gen)?;
        }
        
        self.wi.mmap_view = None;

        Ok(())
    }

    /// Returns the cluster ID of the MMV file
    pub fn cluster_id(&self) -> u32 {
        self.cluster_id
    }

    /// Returns the absolute filesystem path of the MMV file
    pub fn mmv_path(&self) -> &Path {
        self.mmv_path.as_path()
    }
}

fn three_way_split(view: MmapViewSync, mid_idx: usize, mid_len: usize) -> io::Result<(MmapViewSync, MmapViewSync, MmapViewSync)> {
    let (left_view, mid_right_view) = view.split_at(mid_idx).unwrap();
    let (mid_view, right_view) = mid_right_view.split_at(mid_len).unwrap();
    Ok((left_view, mid_view, right_view))
}

#[test]
fn test_mmv_header() {
    use byteorder::ReadBytesExt;
    use rand::{thread_rng, Rng};

    let cluster_id = thread_rng().gen::<u32>();
    let flags = PROCESS | SENTINEL;
    let mut client = Client::new_custom("mmv_header_test", flags, cluster_id).unwrap();
    client.begin_metrics(0).unwrap().export().unwrap();

    let mut file = File::open(client.mmv_path()).unwrap();
    let mut header = Vec::new();
    assert!(
        (HDR_LEN + 2*TOC_BLOCK_LEN) as usize
        <= file.read_to_end(&mut header).unwrap()
    );
    
    let mut cursor = Cursor::new(header);
    
    // test "MMV\0"
    assert_eq!('M' as u8, cursor.read_u8().unwrap());
    assert_eq!('M' as u8, cursor.read_u8().unwrap());
    assert_eq!('V' as u8, cursor.read_u8().unwrap());
    assert_eq!(0, cursor.read_u8().unwrap());
    // test version number
    assert_eq!(1, cursor.read_u32::<Endian>().unwrap());
    // test generation
    assert_eq!(
        cursor.read_i64::<Endian>().unwrap(),
        cursor.read_i64::<Endian>().unwrap()
    );
    // test no. of toc blocks
    assert!(2 <= cursor.read_i32::<Endian>().unwrap());
    // test flags
    assert_eq!(flags.bits(), cursor.read_u32::<Endian>().unwrap());
    // test pid
    assert_eq!(get_process_id(), cursor.read_i32::<Endian>().unwrap());
    // cluster id
    assert_eq!(client.cluster_id(), cursor.read_u32::<Endian>().unwrap());
}

#[test]
fn test_mmv_dir() {
    let pcp_root = get_pcp_root();
    let mmv_dir = get_mmv_dir().unwrap();
    let tmp_dir = PathBuf::from(
        env::var_os(PCP_TMP_DIR_KEY)
        .expect(&format!("{} not set", PCP_TMP_DIR_KEY))
    );

    assert!(mmv_dir.is_dir());
    assert_eq!(mmv_dir, pcp_root.join(tmp_dir).join(MMV_DIR_SUFFIX));
}

#[test]
fn test_init_pcp_conf() {
    let conf_keys = vec!(
        "PCP_VERSION",
        "PCP_USER",
        "PCP_GROUP",
        "PCP_PLATFORM",
        "PCP_PLATFORM_PATHS",
        "PCP_ETC_DIR",
        "PCP_SYSCONF_DIR",
        "PCP_SYSCONFIG_DIR",
        "PCP_RC_DIR",
        "PCP_BIN_DIR",
        "PCP_BINADM_DIR",
        "PCP_LIB_DIR",
        "PCP_LIB32_DIR",
        "PCP_SHARE_DIR",
        "PCP_INC_DIR",
        "PCP_MAN_DIR",
        "PCP_PMCDCONF_PATH",
        "PCP_PMCDOPTIONS_PATH",
        "PCP_PMCDRCLOCAL_PATH",
        "PCP_PMPROXYOPTIONS_PATH",
        "PCP_PMWEBDOPTIONS_PATH",
        "PCP_PMMGROPTIONS_PATH",
        "PCP_PMIECONTROL_PATH",
        "PCP_PMSNAPCONTROL_PATH",
        "PCP_PMLOGGERCONTROL_PATH",
        "PCP_PMDAS_DIR",
        "PCP_RUN_DIR",
        "PCP_PMDAS_DIR",
        "PCP_LOG_DIR",
        "PCP_TMP_DIR",
        "PCP_TMPFILE_DIR",
        "PCP_DOC_DIR",
        "PCP_DEMOS_DIR",
    );

    let pcp_root = get_pcp_root();
    if init_pcp_conf(&pcp_root).is_ok() {
        for key in conf_keys.iter() {
            env::var(key).expect(&format!("{} not set", key));
        }
    }
}
