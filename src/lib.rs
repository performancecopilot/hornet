extern crate byteorder;
extern crate memmap;
extern crate regex;
extern crate time;
#[macro_use] extern crate bitflags;
#[macro_use] extern crate lazy_static;
#[cfg(test)] extern crate rand;
#[cfg(unix)] extern crate nix;
#[cfg(windows)] extern crate kernel32;

use byteorder::{LittleEndian, WriteBytesExt};
use memmap::{Mmap, Protection};
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

const HDR_LEN: usize = 40;
const TOC_BLOCK_LEN: usize = 16;
const CLUSTER_ID_BIT_LEN: usize = 12;

static PCP_TMP_DIR_KEY: &'static str = "PCP_TMP_DIR";
static MMV_DIR_SUFFIX: &'static str = "mmv";

type Endian = LittleEndian;

#[cfg(unix)]
fn get_process_id() -> i32 {
    nix::unistd::getpid()
}

#[cfg(windows)]
fn get_process_id() -> i32 {
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
    println!("mmv_dir = {}", mmv_dir.to_str().unwrap());
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
    gen: i64,
    gen2off: u64
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
    pub fn export(&self) -> io::Result<()> {
        let mut file = OpenOptions::new().read(true).write(true).create(true)
            .open(&self.mmv_path)?;
        let mmv_size = HDR_LEN + 2*TOC_BLOCK_LEN;
        file.write(&vec![0; mmv_size])?;

        let mut mmap = Mmap::open(&file, Protection::ReadWrite)?;
        let mut cursor = Cursor::new(unsafe { mmap.as_mut_slice() });

        let writer_info = self.write_mmv_header(&mut cursor)?;
        self.write_metric_toc_block(&mut cursor)?;
        self.write_values_toc_block(&mut cursor)?;
        self.unlock_mmv_header(&mut cursor, &writer_info)
    }

    fn write_mmv_header(&self, cursor: &mut Cursor<&mut [u8]>)
        -> io::Result<MMVWriterInfo> {
        let mut writer_info = MMVWriterInfo {
            gen: 0,
            gen2off: 0
        };
    
        // MMV\0
        cursor.write_all(CString::new("MMV")?.to_bytes_with_nul())?;
        // version
        cursor.write_u32::<Endian>(1)?;
        // generation1
        let gen = time::now().to_timespec().sec;
        cursor.write_i64::<Endian>(gen)?;
        writer_info.gen = gen;
        // generation2
        writer_info.gen2off = cursor.position();
        cursor.write_i64::<Endian>(0)?;
        // no. of toc blocks
        cursor.write_i32::<Endian>(2)?;
        // flags
        cursor.write_u32::<Endian>(self.flags.bits())?;
        // pid
        cursor.write_i32::<Endian>(get_process_id())?;
        // cluster id
        cursor.write_u32::<Endian>(self.cluster_id)?;

        Ok(writer_info)
    }

    fn write_metric_toc_block(&self, cursor: &mut Cursor<&mut [u8]>)
         -> io::Result<()> {
        // section type
        cursor.write_u32::<Endian>(3)?;
        // no. of entries
        cursor.write_u32::<Endian>(0)?;
        // section offset
        cursor.write_u64::<Endian>(0)
    }

    fn write_values_toc_block(&self, cursor: &mut Cursor<&mut [u8]>)
         -> io::Result<()> {
        // section type
        cursor.write_u32::<Endian>(4)?;
        // no. of entries
        cursor.write_u32::<Endian>(0)?;
        // section offset
        cursor.write_u64::<Endian>(0)
    }

    fn unlock_mmv_header(&self, cursor: &mut Cursor<&mut [u8]>,
        writer_info: &MMVWriterInfo)-> io::Result<()> {
        cursor.set_position(writer_info.gen2off);
        cursor.write_i64::<Endian>(writer_info.gen)
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

#[test]
fn test_mmv_header() {
    use byteorder::ReadBytesExt;
    use rand::Rng;

    let cluster_id = rand::thread_rng().gen::<u32>();
    let flags = PROCESS | SENTINEL;
    let client = Client::new_custom("mmv_header_test", flags, cluster_id).unwrap();
    client.export().unwrap();

    let mut file = File::open(client.mmv_path()).unwrap();
    let mut header = Vec::new();
    assert!(
        HDR_LEN + 2*TOC_BLOCK_LEN
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
