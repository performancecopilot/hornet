use crate::byteio::WriteBytesExt;
use bitflags::bitflags;
use memmap2::MmapMut;
use std::env;
use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs;
use std::fs::{File, OpenOptions};
use std::io;
use std::io::prelude::*;
use std::io::{BufReader, Cursor};
use std::path::{Path, PathBuf, MAIN_SEPARATOR};
use std::process;
use std::str;
use std::time::{SystemTime, UNIX_EPOCH};

use super::mmv::Version;
use super::{
    CLUSTER_ID_BIT_LEN, HDR_LEN, INDOM_BLOCK_LEN, INSTANCE_BLOCK_LEN_MMV1, INSTANCE_BLOCK_LEN_MMV2,
    METRIC_BLOCK_LEN_MMV1, METRIC_BLOCK_LEN_MMV2, STRING_BLOCK_LEN, TOC_BLOCK_LEN, VALUE_BLOCK_LEN,
};

pub mod metric;
use self::metric::{MMVWriter, MMVWriterState, MmapView};

static PCP_TMP_DIR_KEY: &'static str = "PCP_TMP_DIR";
static MMV_DIR_SUFFIX: &'static str = "mmv";

fn get_process_id() -> i32 {
    process::id() as i32
}

#[cfg(unix)]
fn osstr_from_bytes(slice: &[u8]) -> Option<&OsStr> {
    use std::os::unix::ffi::OsStrExt;
    Some(OsStr::from_bytes(slice))
}

/// Windows stores `OsStr` as WTF-8 and has no borrowed constructor from bytes,
/// so the only route from `&[u8]` to `&OsStr` is via `&str`. Bytes that aren't
/// valid UTF-8 are rejected rather than assumed.
#[cfg(windows)]
fn osstr_from_bytes(slice: &[u8]) -> Option<&OsStr> {
    str::from_utf8(slice).ok().map(OsStr::new)
}

fn get_pcp_root() -> PathBuf {
    match env::var_os("PCP_DIR") {
        Some(val) => PathBuf::from(val),
        None => PathBuf::from(MAIN_SEPARATOR.to_string()),
    }
}

fn init_pcp_conf(pcp_root: &Path) -> io::Result<()> {
    /* attempt to load variables from pcp_root/etc/pcp.conf into environment.
    if pcp_root/etc/pcp.conf is not a file, can't be read, or parsing it
    fails, we *don't* return the error */
    if let Ok(values) = parse_pcp_conf(pcp_root.join("etc").join("pcp.conf")) {
        apply_pcp_conf(values);
    }

    /* attempt to load variables from pcp_root/$PCP_CONF into environment.
    if pcp_root/$PCP_CONF is not a file, can't be read, or parsing it
    fails, we *do* return the error */
    let pcp_conf = pcp_root.join(env::var_os("PCP_CONF").unwrap_or(OsString::new()));
    let values = parse_pcp_conf(pcp_conf)?;
    apply_pcp_conf(values);
    Ok(())
}

/// Parses one `PCP_VARIABLE_NAME=value` line, per the syntax in
/// `man 5 pcp.conf`: no space around the `=`, and values are unquoted and
/// may contain spaces.
///
/// Returns `None` for anything else.
fn parse_pcp_conf_line(line: &[u8]) -> Option<(&[u8], &[u8])> {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    let eq = line.iter().position(|&b| b == b'=')?;
    let (key, val) = (&line[..eq], &line[eq + 1..]);

    let suffix = key.strip_prefix(b"PCP_")?;
    if suffix.is_empty()
        || !suffix
            .iter()
            .all(|&b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return None;
    }

    let is_quote = |b: u8| b == b'"' || b == b'\'';
    if val.first().map_or(false, |&b| is_quote(b)) {
        return None;
    }

    Some((key, val))
}

fn parse_pcp_conf<P: AsRef<Path>>(conf_path: P) -> io::Result<Vec<(OsString, OsString)>> {
    let pcp_conf = File::open(conf_path)?;
    let mut buf_reader = BufReader::new(pcp_conf);

    let mut values = Vec::new();
    let mut line = Vec::new();
    while buf_reader.read_until(b'\n', &mut line)? > 0 {
        if let Some((key, val)) = parse_pcp_conf_line(&line) {
            if let (Some(key), Some(val)) = (osstr_from_bytes(key), osstr_from_bytes(val)) {
                values.push((key.to_os_string(), val.to_os_string()));
            }
        }
        line.clear();
    }

    Ok(values)
}

fn unset_pcp_conf_values(
    values: impl IntoIterator<Item = (OsString, OsString)>,
    is_set: impl Fn(&OsStr) -> bool,
) -> Vec<(OsString, OsString)> {
    values.into_iter().filter(|(key, _)| !is_set(key)).collect()
}

fn apply_pcp_conf(values: Vec<(OsString, OsString)>) {
    for (key, val) in unset_pcp_conf_values(values, |key| env::var_os(key).is_some()) {
        env::set_var(key, val);
    }
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
    #[derive(Clone, Copy)]
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

        if self.contains(MMVFlags::NOPREFIX) {
            write!(f, "no prefix")?;
            prev_flag = true;
        }

        if self.contains(MMVFlags::PROCESS) {
            if prev_flag {
                write!(f, ",")?;
            }
            write!(f, "process")?;
            prev_flag = true;
        }

        if self.contains(MMVFlags::SENTINEL) {
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
    mmv_path: PathBuf,
}

impl Client {
    /// Creates a new client with `PROCESS` flag and `0` cluster ID
    pub fn new(name: &str) -> io::Result<Client> {
        Client::new_custom(name, MMVFlags::PROCESS, 0)
    }

    /// Creates a new client with custom flags and cluster ID
    ///
    /// Note that only the 12 least significant bits of `cluster_id` will be
    /// used.
    pub fn new_custom(name: &str, flags: MMVFlags, cluster_id: u32) -> io::Result<Client> {
        let mmv_path = get_mmv_dir()?.join(name);
        let cluster_id = cluster_id & ((1 << CLUSTER_ID_BIT_LEN) - 1);

        Ok(Client {
            flags: flags,
            cluster_id: cluster_id,
            mmv_path: mmv_path,
        })
    }

    /// Exports metrics to an MMV file at `mmv_path`
    ///
    /// If an MMV file is already present at `mmv_path`, it's overwritten
    /// with the newer metrics.
    pub fn export(&self, metrics: &mut [&mut dyn MMVWriter]) -> io::Result<()> {
        let mut ws = MMVWriterState::new();

        let mut mmv_ver = Version::V1;
        for m in metrics.iter() {
            if m.has_mmv2_string() {
                mmv_ver = Version::V2;
                break;
            }
        }

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

        let hdr_toc_len = HDR_LEN + TOC_BLOCK_LEN * ws.n_toc;

        ws.indom_sec_off = hdr_toc_len;
        ws.instance_sec_off = ws.indom_sec_off + INDOM_BLOCK_LEN * ws.n_indoms;

        let (instance_blk_len, metric_blk_len) = match mmv_ver {
            Version::V1 => (INSTANCE_BLOCK_LEN_MMV1, METRIC_BLOCK_LEN_MMV1),
            Version::V2 => (INSTANCE_BLOCK_LEN_MMV2, METRIC_BLOCK_LEN_MMV2),
        };

        ws.metric_sec_off = ws.instance_sec_off + instance_blk_len * ws.n_instances;
        ws.value_sec_off = ws.metric_sec_off + metric_blk_len * ws.n_metrics;
        ws.string_sec_off = ws.value_sec_off + VALUE_BLOCK_LEN * ws.n_values;

        let mmv_size = (ws.string_sec_off + STRING_BLOCK_LEN * ws.n_strings) as usize;

        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&self.mmv_path)?;

        file.write_all(&vec![0; mmv_size])?;

        // Safety: the file was just created and zero-filled above, and no
        // other writer has it open.
        let mmap_view = MmapView::whole(unsafe { MmapMut::map_mut(&file)? });
        ws.mmap_view = Some(mmap_view.clone());

        ws.flags = self.flags.bits();
        ws.cluster_id = self.cluster_id;

        let mut guard = mmap_view.lock_whole();
        let mut c = Cursor::new(&mut guard[..]);

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
        c.write_i64(ws.gen)?;

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

fn write_mmv_header(
    ws: &mut MMVWriterState,
    c: &mut Cursor<&mut [u8]>,
    mmv_ver: Version,
) -> io::Result<()> {
    // MMV\0
    c.write_all(b"MMV\0")?;

    // version
    match mmv_ver {
        Version::V1 => c.write_u32(1)?,
        Version::V2 => c.write_u32(2)?,
    }

    // generation1
    ws.gen = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    c.write_i64(ws.gen)?;
    // generation2
    ws.gen2_off = c.position();
    c.write_i64(0)?;
    // no. of toc blocks
    c.write_u32(ws.n_toc as u32)?;
    // flags
    c.write_u32(ws.flags)?;
    // pid
    c.write_i32(get_process_id())?;
    // cluster id
    c.write_u32(ws.cluster_id)
}

fn write_toc_block(
    sec: u32,
    entries: u32,
    sec_off: u64,
    c: &mut Cursor<&mut [u8]>,
) -> io::Result<()> {
    if entries > 0 {
        // section type
        c.write_u32(sec)?;
        // no. of entries
        c.write_u32(entries)?;
        // section offset
        c.write_u64(sec_off)?;
    }
    Ok(())
}

#[test]
fn test_mmv_header() {
    use crate::byteio::ReadBytesExt;
    use rand::Rng;

    let cluster_id = rand::thread_rng().gen::<u32>();
    let flags = MMVFlags::PROCESS | MMVFlags::SENTINEL;
    let client = Client::new_custom("mmv_header_test", flags, cluster_id).unwrap();

    client.export(&mut []).unwrap();

    let mut file = File::open(client.mmv_path()).unwrap();
    let mut header = Vec::new();
    assert!(HDR_LEN as usize <= file.read_to_end(&mut header).unwrap());

    let mut cursor = Cursor::new(header);

    // test "MMV\0"
    assert_eq!('M' as u8, cursor.read_u8().unwrap());
    assert_eq!('M' as u8, cursor.read_u8().unwrap());
    assert_eq!('V' as u8, cursor.read_u8().unwrap());
    assert_eq!(0, cursor.read_u8().unwrap());
    // test version number
    assert_eq!(1, cursor.read_u32().unwrap());
    // test generation
    assert_eq!(cursor.read_i64().unwrap(), cursor.read_i64().unwrap());
    // test no. of toc blocks
    assert_eq!(0, cursor.read_i32().unwrap());
    // test flags
    assert_eq!(flags.bits(), cursor.read_u32().unwrap());
    // test pid
    assert_eq!(get_process_id(), cursor.read_i32().unwrap());
    // cluster id
    assert_eq!(client.cluster_id(), cursor.read_u32().unwrap());
}

#[test]
fn test_mmv_dir() {
    let pcp_root = get_pcp_root();
    let mmv_dir = get_mmv_dir().unwrap();
    let tmp_dir =
        PathBuf::from(env::var_os(PCP_TMP_DIR_KEY).expect(&format!("{} not set", PCP_TMP_DIR_KEY)));

    assert!(mmv_dir.is_dir());
    assert_eq!(mmv_dir, pcp_root.join(tmp_dir).join(MMV_DIR_SUFFIX));
}

#[test]
fn test_parse_pcp_conf_fixture() {
    let tmp = tempfile::tempdir().unwrap();
    let conf = tmp.path().join("pcp.conf");
    fs::write(
        &conf,
        b"# comments and blank lines are ignored\r\n\
          PCP_TMP_DIR=/tmp/from-file\r\n\
          PCP_USER=hornet\r\n\
          PCP_EMPTY=\r\n\
          PCP_QUOTED=\"not accepted\"\r\n\
          NOT_PCP=ignored\r\n",
    )
    .unwrap();

    let values = parse_pcp_conf(&conf).unwrap();
    assert_eq!(
        values,
        vec![
            (
                OsString::from("PCP_TMP_DIR"),
                OsString::from("/tmp/from-file"),
            ),
            (OsString::from("PCP_USER"), OsString::from("hornet")),
            (OsString::from("PCP_EMPTY"), OsString::new()),
        ]
    );

    let values_to_set = unset_pcp_conf_values(values, |key| key == OsStr::new(PCP_TMP_DIR_KEY));
    assert_eq!(
        values_to_set,
        vec![
            (OsString::from("PCP_USER"), OsString::from("hornet")),
            (OsString::from("PCP_EMPTY"), OsString::new()),
        ]
    );
}
