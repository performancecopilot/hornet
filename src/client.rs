
use byteorder::WriteBytesExt;
use memmap::{Mmap, MmapViewSync, Protection};
use regex::bytes::Regex;
use std::env;
use std::ffi::{CString, OsStr, OsString};
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
    MAX_STRINGS_PER_METRIC,
    TOC_BLOCK_COUNT
};
use super::metric::{Metric, MetricType, MMVMetric, STRING_METRIC_TYPE_CODE};

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
    n_metrics: u64,
    n_strings: u64,
    gen: i64,
    gen2off: u64,
    metric_sec_off: u64,
    value_sec_off: u64,
    string_sec_off: u64,
    metric_idx: u64,
    n_strings_off: u64
}

impl MMVWriterInfo {
    fn new() -> Self {
        MMVWriterInfo {
            mmap_view: None,
            n_metrics: 0,
            n_strings: 0,
            gen: 0,
            gen2off: 0,
            metric_sec_off: 0,
            value_sec_off: 0,
            string_sec_off: 0,
            metric_idx: 0,
            n_strings_off: 0
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

    pub fn begin(&mut self, n_metrics: u64) -> io::Result<&mut Client> {
        self.wi.n_metrics = n_metrics;
        let mmv_size = (
            HDR_LEN + TOC_BLOCK_COUNT*TOC_BLOCK_LEN
            + self.wi.n_metrics*(METRIC_BLOCK_LEN + VALUE_BLOCK_LEN)
            + self.wi.n_metrics*MAX_STRINGS_PER_METRIC*STRING_BLOCK_LEN
        ) as usize;
        
        let mut file = OpenOptions::new().read(true).write(true).create(true)
            .open(&self.mmv_path)?;
        file.write(&vec![0; mmv_size])?;

        let mut mmap_view = Mmap::open(&file, Protection::ReadWrite)?
            .into_view_sync();

        /*
            The layout of the MMV is as follows:

            --- MMV Header
            
            --- Metrics TOC Block
            
            --- Values TOC Block

            --- Strings TOC Block

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
            self.write_metric_toc_block(&mut cur)?;
            self.write_values_toc_block(&mut cur)?;
            self.write_strings_toc_block(&mut cur)?;
        }

        self.wi.mmap_view = Some(mmap_view);
        Ok(self)
    }

    fn write_mmv_header(&mut self, c: &mut Cursor<&mut [u8]>) -> io::Result<()> {    
        // MMV\0
        c.write_all(CString::new("MMV")?.to_bytes_with_nul())?;
        // version
        c.write_u32::<Endian>(1)?;
        // generation1
        self.wi.gen = time::now().to_timespec().sec;
        c.write_i64::<Endian>(self.wi.gen)?;
        // generation2
        self.wi.gen2off = c.position();
        c.write_i64::<Endian>(0)?;
        // no. of toc blocks
        c.write_u32::<Endian>(TOC_BLOCK_COUNT as u32)?;
        // flags
        c.write_u32::<Endian>(self.flags.bits())?;
        // pid
        c.write_i32::<Endian>(get_process_id())?;
        // cluster id
        c.write_u32::<Endian>(self.cluster_id)
    }
    
    fn write_metric_toc_block(&mut self, c: &mut Cursor<&mut [u8]>) -> io::Result<()> {
        // section type
        c.write_u32::<Endian>(3)?;
        // no. of entries
        c.write_u32::<Endian>(self.wi.n_metrics as u32)?;
        // section offset
        self.wi.metric_sec_off = HDR_LEN + TOC_BLOCK_LEN*TOC_BLOCK_COUNT;
        c.write_u64::<Endian>(self.wi.metric_sec_off)
    }

    fn write_values_toc_block(&mut self, c: &mut Cursor<&mut [u8]>) -> io::Result<()> {
        // section type
        c.write_u32::<Endian>(4)?;
        // no. of entries
        c.write_u32::<Endian>(self.wi.n_metrics as u32)?;
        // section offset
        self.wi.value_sec_off = self.wi.metric_sec_off + METRIC_BLOCK_LEN*self.wi.n_metrics;
        c.write_u64::<Endian>(self.wi.value_sec_off)
    }

    fn write_strings_toc_block(&mut self, c: &mut Cursor<&mut [u8]>) -> io::Result<()> {
        // section type
        c.write_u32::<Endian>(5)?;
        // no. of entries
        self.wi.n_strings_off = c.position();
        c.write_u32::<Endian>(self.wi.n_strings as u32)?;
        // section offset
        self.wi.string_sec_off = self.wi.value_sec_off + VALUE_BLOCK_LEN*self.wi.n_metrics;
        c.write_u64::<Endian>(self.wi.string_sec_off)
    }

    pub fn register_metric<T: MetricType + Clone>(&mut self, m: &mut MMVMetric<T>) -> io::Result<&mut Client> {

        // TODO: return custom error instead of panicing
        assert!(self.wi.metric_idx < self.wi.n_metrics);

        let (mut value_offset, mut value_size) = (0, 0);

        { // write metric, value, string blocks

            let mmap_view = self.wi.mmap_view.as_mut().unwrap();
            let mut c = Cursor::new(unsafe { mmap_view.as_mut_slice() });

            let i = self.wi.metric_idx;

            // metric block
            let metric_block_off = self.wi.metric_sec_off + i*METRIC_BLOCK_LEN;
            c.set_position(metric_block_off);
            // name
            c.write_all(CString::new(m.name())?.to_bytes_with_nul())?;
            c.set_position(metric_block_off + METRIC_NAME_MAX_LEN);
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
            // record short and long help offset
            let shorthelp_off_off = c.position();
            c.write_u64::<Endian>(0)?;
            let longhelp_off_off = c.position();
            c.write_u64::<Endian>(0)?;

            // value block
            let value_block_off = self.wi.value_sec_off + i*VALUE_BLOCK_LEN;
            c.set_position(value_block_off);
            let mut string_val_off_off = None;
            if type_code == STRING_METRIC_TYPE_CODE {
                // value
                c.write_u64::<Endian>(0)?;
                // extra (string offset)
                string_val_off_off = Some(c.position());
                c.write_u64::<Endian>(0)?;
            } else {
                value_offset = c.position() as usize;
                value_size = NUMERIC_VALUE_SIZE;

                // value
                m.write_val(&mut c)?;
                // extra (string offset)
                c.write_u64::<Endian>(0)?;
            }
            // offset to metric block
            c.write_u64::<Endian>(metric_block_off)?;
            // offset to instance block
            c.write_u64::<Endian>(0)?;

            // string block
            let mut string_block_off = self.wi.string_sec_off
                + self.wi.n_strings*STRING_BLOCK_LEN;

            // short help
            if m.shorthelp().len() > 0 {
                c.set_position(shorthelp_off_off);
                c.write_u64::<Endian>(string_block_off)?;

                c.set_position(string_block_off);
                c.write_all(CString::new(m.shorthelp())?.to_bytes_with_nul())?;

                self.wi.n_strings += 1;
                string_block_off += STRING_BLOCK_LEN;
            }

            // long help
            if m.longhelp().len() > 0 {
                c.set_position(longhelp_off_off);
                c.write_u64::<Endian>(string_block_off)?;

                c.set_position(string_block_off);
                c.write_all(CString::new(m.longhelp())?.to_bytes_with_nul())?;

                self.wi.n_strings += 1;
                string_block_off += STRING_BLOCK_LEN;
            }

            // string value
            match string_val_off_off {
                Some(off_off) => {
                    c.set_position(off_off);
                    c.write_u64::<Endian>(string_block_off)?;

                    c.set_position(string_block_off);
                    m.write_val(&mut c)?;

                    value_offset = string_block_off as usize;
                    value_size = STRING_BLOCK_LEN as usize;

                    self.wi.n_strings += 1;
                },
                None => {}
            }

            // update string count in string TOC block
            c.set_position(self.wi.n_strings_off);
            c.write_u32::<Endian>(self.wi.n_strings as u32)?;

        }

        { // set mmap_view for metric

            let mmap_view = unsafe {
                self.wi.mmap_view.as_mut().unwrap().clone()
            };
            let (_, value_mmap_view, _) =
                three_way_split(mmap_view, value_offset, value_size)?;
            m.set_mmap_view(value_mmap_view);

        }

        self.wi.metric_idx += 1;
        Ok(self)
    }

    pub fn export(&mut self) -> io::Result<()> {
        {
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
    client.begin(0).unwrap().export().unwrap();

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
