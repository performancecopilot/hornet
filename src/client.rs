
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
use super::metric::MMVMetric;
use time;

use super::{
    Endian,
    CLUSTER_ID_BIT_LEN,
    HDR_LEN,
    TOC_BLOCK_LEN,
    METRIC_BLOCK_LEN,
    VALUE_BLOCK_LEN,
    STRING_BLOCK_LEN,
    METRIC_NAME_MAX_LEN,
    MIN_STRINGS_PER_METRIC
};
use super::metric::STRING_METRIC_TYPE_CODE;

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
    n_metrics: u64,
    n_strings: u64,
    gen: i64,
    gen2off: u64,
    metric_sec_off: u64,
    value_sec_off: u64,
    string_sec_off: u64,
    string_vals_off: u64
}

impl MMVWriterInfo {
    fn new() -> Self {
        MMVWriterInfo {
            n_metrics: 0,
            n_strings: 0,
            gen: 0,
            gen2off: 0,
            metric_sec_off: 0,
            value_sec_off: 0,
            string_sec_off: 0,
            string_vals_off: 0
        }
    }
}

/// Client used to export metrics
pub struct Client {
    flags: MMVFlags,
    cluster_id: u32,
    mmv_path: PathBuf
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
            mmv_path: mmv_path
        })
    }
    
    /// Exports metrics by writing to an MMV file
    pub fn export(&self, mut metrics: &mut [&mut MMVMetric]) -> io::Result<()> {
        let mut wi = MMVWriterInfo::new();
        wi.n_metrics = metrics.len() as u64;
        wi.n_strings = wi.n_metrics*MIN_STRINGS_PER_METRIC
            + metrics.iter()
                .filter(|m| m.type_code() == STRING_METRIC_TYPE_CODE)
                .count() as u64;
        let mmv_size = (
            HDR_LEN + 3*TOC_BLOCK_LEN
            + wi.n_metrics*(METRIC_BLOCK_LEN + VALUE_BLOCK_LEN)
            + wi.n_strings*STRING_BLOCK_LEN
        ) as usize;
        
        let mut file = OpenOptions::new().read(true).write(true).create(true)
            .open(&self.mmv_path)?;
        file.write(&vec![0; mmv_size])?;

        let mmap_view = Mmap::open(&file, Protection::ReadWrite)?
            .into_view_sync();

        let mut mmv_view = unsafe { mmap_view.clone() };
        let mut c = Cursor::new(unsafe { mmv_view.as_mut_slice() });

        /*
            The layout of the MMV is as follows:
            --- MMV Header
            --- Metrics TOC Block
            --- Values TOC Block
            --- Strings TOC Block
            --- Metrics Block
            --- Values Block
            --- Strings Block
                --- Help text (short and long) block
                --- String values block
            
            After writing, every metric is given ownership
            of the respective memory-mapped slice that contains
            the metric's value. This is to ensure that the metric
            is *only* able to write to it's value slice when updating
            it's value.
        */

        self.write_mmv_header(&mut c, &mut wi)?;
        self.write_metric_toc_block(&mut c, &mut wi)?;
        self.write_values_toc_block(&mut c, &mut wi)?;
        self.write_strings_toc_block(&mut c, &mut wi)?;
        self.write_metrics(&mut c, &mut wi, &mut metrics)?;
        self.unlock_mmv_header(&mut c, &wi)?;

        self.split_mmap_views(unsafe { mmap_view.clone() }, &wi, &mut metrics)
    }

    fn write_mmv_header(&self, cursor: &mut Cursor<&mut [u8]>,
        wi: &mut MMVWriterInfo) -> io::Result<()> {    
        // MMV\0
        cursor.write_all(CString::new("MMV")?.to_bytes_with_nul())?;
        // version
        cursor.write_u32::<Endian>(1)?;
        // generation1
        wi.gen = time::now().to_timespec().sec;
        cursor.write_i64::<Endian>(wi.gen)?;
        // generation2
        wi.gen2off = cursor.position();
        cursor.write_i64::<Endian>(0)?;
        // no. of toc blocks
        cursor.write_i32::<Endian>(3)?;
        // flags
        cursor.write_u32::<Endian>(self.flags.bits())?;
        // pid
        cursor.write_i32::<Endian>(get_process_id())?;
        // cluster id
        cursor.write_u32::<Endian>(self.cluster_id)
    }

    fn write_metric_toc_block(&self, c: &mut Cursor<&mut [u8]>,
        wi: &mut MMVWriterInfo) -> io::Result<()> {
        // section type
        c.write_u32::<Endian>(3)?;
        // no. of entries
        c.write_u32::<Endian>(wi.n_metrics as u32)?;
        // section offset
        wi.metric_sec_off = HDR_LEN + TOC_BLOCK_LEN*3;
        c.write_u64::<Endian>(wi.metric_sec_off)
    }

    fn write_values_toc_block(&self, c: &mut Cursor<&mut [u8]>,
        wi: &mut MMVWriterInfo) -> io::Result<()> {
        // section type
        c.write_u32::<Endian>(4)?;
        // no. of entries
        c.write_u32::<Endian>(wi.n_metrics as u32)?;
        // section offset
        wi.value_sec_off = wi.metric_sec_off
            + METRIC_BLOCK_LEN*wi.n_metrics;
        c.write_u64::<Endian>(wi.value_sec_off)
    }

    fn write_strings_toc_block(&self, c: &mut Cursor<&mut [u8]>,
        wi: &mut MMVWriterInfo) -> io::Result<()> {
        // section type
        c.write_u32::<Endian>(5)?;
        // no. of entries
        c.write_u32::<Endian>(2*wi.n_metrics as u32)?;
        // section offset
        wi.string_sec_off = wi.value_sec_off
            + VALUE_BLOCK_LEN*wi.n_metrics;
        wi.string_vals_off = wi.string_sec_off
            + STRING_BLOCK_LEN*2*wi.n_metrics;
        c.write_u64::<Endian>(wi.string_sec_off)
    }

    fn write_metrics(&self, mut c: &mut Cursor<&mut [u8]>, wi: &mut MMVWriterInfo,
        metrics: &mut [&mut MMVMetric]) -> io::Result<()> {

        let mut string_metric_idx = 0;

        // metric, value, string blocks
        for (i, m) in metrics.iter_mut().enumerate() {
            let i = i as u64;
            
            // metric block
            let metric_block_off = wi.metric_sec_off + i*METRIC_BLOCK_LEN;
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
            let longhelp_off_off = shorthelp_off_off + 8;

            // value block
            let value_block_off = wi.value_sec_off + i*VALUE_BLOCK_LEN;
            c.set_position(value_block_off);
            // value (for non-string values) and extra (for string values)
            let mut string_val_off_off = None;
            if type_code == STRING_METRIC_TYPE_CODE {
                c.write_u64::<Endian>(0)?;
                string_val_off_off = Some(c.position());
                c.write_u64::<Endian>(0)?;
            } else {
                m.write_val(&mut c)?;
                c.write_u64::<Endian>(0)?;
            }
            // offset to metric block
            c.write_u64::<Endian>(metric_block_off)?;
            // offset to instance block
            c.write_u64::<Endian>(0)?;

            // string block
            let string_block_off = wi.string_sec_off
                + i*MIN_STRINGS_PER_METRIC*STRING_BLOCK_LEN;

            // short help
            c.set_position(shorthelp_off_off);
            let shorthelp_off = string_block_off;
            c.write_u64::<Endian>(string_block_off)?;
            c.set_position(shorthelp_off);
            c.write_all(CString::new(m.shorthelp())?.to_bytes_with_nul())?;

            // long help
            c.set_position(longhelp_off_off);
            let longhelp_off = shorthelp_off + STRING_BLOCK_LEN;
            c.write_u64::<Endian>(longhelp_off)?;
            c.set_position(longhelp_off);
            c.write_all(CString::new(m.longhelp())?.to_bytes_with_nul())?;

            // string val
            match string_val_off_off {
                Some(off_off) => {
                    c.set_position(off_off);
                    let string_val_off = wi.string_vals_off
                        + string_metric_idx*STRING_BLOCK_LEN;
                    c.write_u64::<Endian>(string_val_off)?;
                    c.set_position(string_val_off);
                    m.write_val(&mut c)?;
                    string_metric_idx += 1;
                },
                None => {}
            }
        }

        Ok(())
    }

    fn unlock_mmv_header(&self, c: &mut Cursor<&mut [u8]>, wi: &MMVWriterInfo) -> io::Result<()> {
        c.set_position(wi.gen2off);
        c.write_i64::<Endian>(wi.gen)
    }

    fn split_mmap_views(&self, mmap_view: MmapViewSync, wi: &MMVWriterInfo, metrics: &mut [&mut MMVMetric]) -> io::Result<()> {
        let mut right_view = mmap_view;
        let mut left_mid_len = 0;

        // split views for non-string valued metrics first because value blocks
        // are stored before string blocks

        for (i, m) in metrics.iter_mut().enumerate() {
            if m.type_code() != STRING_METRIC_TYPE_CODE {
                let value_block_off = (wi.value_sec_off as usize)
                    + i*(VALUE_BLOCK_LEN as usize);

                let (left_view, mid_view, r_view) =
                    split_view(right_view, value_block_off - left_mid_len, 8)?;
                right_view = r_view;
                left_mid_len += left_view.len() + mid_view.len();

                m.set_mmap_view(mid_view);
            }
        }

        for (i, m) in metrics.iter_mut()
            .filter(|m| m.type_code() == STRING_METRIC_TYPE_CODE)
            .enumerate() {
                
            let string_val_off = (wi.string_vals_off as usize)
                + i*(STRING_BLOCK_LEN as usize);
            
            let (left_view, mid_view, r_view) =
                split_view(right_view, string_val_off - left_mid_len,
                    STRING_BLOCK_LEN as usize)?;
            right_view = r_view;
            left_mid_len += left_view.len() + mid_view.len();

            m.set_mmap_view(mid_view);
        }

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

fn split_view(view: MmapViewSync, mid_idx: usize, mid_len: usize) -> io::Result<(MmapViewSync, MmapViewSync, MmapViewSync)> {
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
    let client = Client::new_custom("mmv_header_test", flags, cluster_id).unwrap();
    client.export(&mut []).unwrap();

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
