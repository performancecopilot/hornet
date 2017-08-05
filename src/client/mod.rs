use byteorder::WriteBytesExt;
use memmap::{Mmap, Protection};
use regex::bytes::Regex;
use std::env;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs;
use std::fs::{File, OpenOptions};
use std::io;
use std::io::{BufReader, Cursor};
use std::io::prelude::*;
use std::path::{MAIN_SEPARATOR, Path, PathBuf};
use std::str;
use time;

use super::mmv::Version;
use super::{
    Endian,
    CLUSTER_ID_BIT_LEN,
    HDR_LEN,
    TOC_BLOCK_LEN,
    VALUE_BLOCK_LEN,
    STRING_BLOCK_LEN,
    INDOM_BLOCK_LEN,
    METRIC_BLOCK_LEN_MMV1,
    INSTANCE_BLOCK_LEN_MMV1,
    METRIC_BLOCK_LEN_MMV2,
    INSTANCE_BLOCK_LEN_MMV2,
};

pub mod metric;
use self::metric::{MMVWriter, MMVWriterState};

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

impl fmt::Display for MMVFlags {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let mut prev_flag = false;

        if self.contains(NOPREFIX)  {
            write!(f, "no prefix")?;
            prev_flag = true;
        }

        if self.contains(PROCESS)  {
            if prev_flag {
                write!(f, ",")?;
            }
            write!(f, "process")?;
            prev_flag = true;
        }

        if self.contains(SENTINEL)  {
            if prev_flag {
                write!(f, ",")?;
            }
            write!(f, "sentinel")?;
            prev_flag = true;
        }

        if !prev_flag {
            write!(f, "(no flags)")?;
        }

        write!(f, " (0x{:x})", self.bits())
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

    pub fn export(&self, metrics: &mut [&mut MMVWriter]) -> io::Result<()> {
        self.export_common(metrics, Version::V1)
    }

    pub fn export2(&self, metrics: &mut [&mut MMVWriter]) -> io::Result<()> {
        self.export_common(metrics, Version::V2)
    }
    
    fn export_common(&self, metrics: &mut [&mut MMVWriter], mmv_ver: Version) -> io::Result<()> {
        let mut ws = MMVWriterState::new();

        for m in metrics.iter() {
            m.register(&mut ws, mmv_ver);
        }

        if ws.n_metrics > 0 {
            ws.n_toc += 2 /* Metric and Value TOC */;
        }

        if ws.n_strings > 0 {
            ws.n_toc += 1 /* String TOC */;
        }

        if ws.n_indoms > 0 {
            ws.n_toc += 2 /* Indom and Instance TOC */;
        }

        /*
            MMV layout:

            -- MMV Header
            
            -- Instance Domain TOC Block
            -- Instances TOC Block
            -- Metrics TOC Block
            -- Values TOC Block
            -- Strings TOC Block

            -- Instance Domain section
            -- Instances section
            -- Metrics section
            -- Values section
            -- Strings section
            
            After writing, every metric is given ownership
            of the respective memory-mapped slice that contains
            the metric's value. This is to ensure that the metric
            is *only* able to write to it's value's slice when updating
            it's value.
        */

        let hdr_toc_len = HDR_LEN + TOC_BLOCK_LEN*ws.n_toc;

        ws.indom_sec_off = hdr_toc_len;
        ws.instance_sec_off =
            ws.indom_sec_off
            + INDOM_BLOCK_LEN*ws.n_indoms;
        
        let (instance_blk_len, metric_blk_len) = match mmv_ver {
            Version::V1 => (INSTANCE_BLOCK_LEN_MMV1, METRIC_BLOCK_LEN_MMV1),
            Version::V2 => (INSTANCE_BLOCK_LEN_MMV2, METRIC_BLOCK_LEN_MMV2)
        };

        ws.metric_sec_off =
            ws.instance_sec_off
            + instance_blk_len*ws.n_instances;
        ws.value_sec_off =
            ws.metric_sec_off
            + metric_blk_len*ws.n_metrics;
        ws.string_sec_off =
            ws.value_sec_off
            + VALUE_BLOCK_LEN*ws.n_values;

        let mmv_size = (
            ws.string_sec_off
            + STRING_BLOCK_LEN*ws.n_strings
        ) as usize;

        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&self.mmv_path)?;

        file.write(&vec![0; mmv_size])?;

        ws.mmap_view = Some(
            Mmap::open(&file, Protection::ReadWrite)?.into_view_sync()
        );

        let mut mmap_view = unsafe { ws.mmap_view.as_mut().unwrap().clone() };
        let mut c = Cursor::new(unsafe { mmap_view.as_mut_slice() });

        ws.flags = self.flags.bits();
        ws.cluster_id = self.cluster_id;
        write_mmv_header(&mut ws, &mut c, mmv_ver)?;

        write_toc_block(1, ws.n_indoms as u32, ws.indom_sec_off, &mut c)?;
        write_toc_block(2, ws.n_instances as u32, ws.instance_sec_off, &mut c)?;
        write_toc_block(3, ws.n_metrics as u32, ws.metric_sec_off, &mut c)?;
        write_toc_block(4, ws.n_values as u32, ws.value_sec_off, &mut c)?;
        write_toc_block(5, ws.n_strings as u32, ws.string_sec_off, &mut c)?;

        for m in metrics.iter_mut() {
            m.write(&mut ws, &mut c, mmv_ver)?;
        }

        // unlock header; has to be done last
        c.set_position(ws.gen2_off);
        c.write_i64::<Endian>(ws.gen)?;
        
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

fn write_mmv_header(ws: &mut MMVWriterState, c: &mut Cursor<&mut [u8]>, mmv_ver: Version) -> io::Result<()> {    
    // MMV\0
    c.write_all(b"MMV\0")?;

    // version
    match mmv_ver {
        Version::V1 => c.write_u32::<Endian>(1)?,
        Version::V2 => c.write_u32::<Endian>(2)?
    }

    // generation1
    ws.gen = time::now().to_timespec().sec;
    c.write_i64::<Endian>(ws.gen)?;
    // generation2
    ws.gen2_off = c.position();
    c.write_i64::<Endian>(0)?;
    // no. of toc blocks
    c.write_u32::<Endian>(ws.n_toc as u32)?;
    // flags
    c.write_u32::<Endian>(ws.flags)?;
    // pid
    c.write_i32::<Endian>(get_process_id())?;
    // cluster id
    c.write_u32::<Endian>(ws.cluster_id)
}

fn write_toc_block(sec: u32, entries: u32, sec_off: u64, c: &mut Cursor<&mut [u8]>) -> io::Result<()> {
    if entries > 0 {
        // section type
        c.write_u32::<Endian>(sec)?;
        // no. of entries
        c.write_u32::<Endian>(entries)?;
        // section offset
        c.write_u64::<Endian>(sec_off)?;
    }
    Ok(())
}

#[test]
fn test_mmv_header() {
    use byteorder::ReadBytesExt;
    use rand::{thread_rng, Rng};

    let cluster_id = thread_rng().gen::<u32>();
    let flags = PROCESS | SENTINEL;
    let client = Client::new_custom("mmv_header_test", flags, cluster_id).unwrap();
    
    client.export(&mut[]).unwrap();

    let mut file = File::open(client.mmv_path()).unwrap();
    let mut header = Vec::new();
    assert!(
        HDR_LEN as usize
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
    assert_eq!(0, cursor.read_i32::<Endian>().unwrap());
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
