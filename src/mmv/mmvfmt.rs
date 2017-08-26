use super::*;
use super::super::client::MMVFlags;
use super::super::client::metric::{Semantics, Unit};
use std::mem;

impl fmt::Display for Header {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        writeln!(f, "Version    = {}", self.version() as u32)?;
        writeln!(f, "Generated  = {}", self.gen1())?;
        writeln!(f, "TOC count  = {}", self.toc_count())?;
        writeln!(f, "Cluster    = {}", self.cluster_id())?;
        writeln!(f, "Process    = {}", self.pid())?;
        writeln!(f, "Flags      = {}", MMVFlags::from_bits_truncate(self.flags()))
    }
}

fn write_indoms(f: &mut fmt::Formatter, indom_toc: &TocBlk, mmv: &MMV) -> fmt::Result {
    writeln!(f, "TOC[{}]: toc offset {}, indoms offset {} ({} entries)",
        indom_toc._toc_index(), indom_toc._mmv_offset(), indom_toc.sec_offset(), indom_toc.entries())?;

    for (offset, indom) in mmv.indom_blks() {
        if let Some(ref indom_id) = *indom.indom() {
            write!(f, "  [{}/{}] {} instances, starting at offset ",
                indom_id, offset, indom.instances())?;
            match *indom.instances_offset() {
                Some(ref instances_offset) => writeln!(f, "{}", instances_offset)?,
                None => writeln!(f, "(no instances)")?
            }
    
            write!(f, "      ")?;
            match *indom.short_help_offset() {
                Some(ref short_help_offset) => {
                    let shortext = mmv.string_blks().get(short_help_offset).unwrap().string();
                    writeln!(f, "shorttext={}", shortext)?;
                }
                None => writeln!(f, "(no shorttext)")?
            }

            write!(f, "      ")?;
            match *indom.long_help_offset() {
                Some(ref long_help_offset) => {
                    let longtext = mmv.string_blks().get(long_help_offset).unwrap().string();
                    writeln!(f, "longtext={}", longtext)?
                }
                None => writeln!(f, "(no longtext)")?
            }
        }
    }

    Ok(())
}

// note: doesn't write newline at the end
fn write_version_specific_string(f: &mut fmt::Formatter, string: &VersionSpecificString, mmv: &MMV) -> fmt::Result {
    match string {
        &VersionSpecificString::String(ref string) => write!(f, "{}", string),
        &VersionSpecificString::Offset(ref offset) => {
            let string = mmv.string_blks().get(offset).unwrap().string();
            write!(f, "{}", string)
        }
    }
}

fn write_instances(f: &mut fmt::Formatter, instance_toc: &TocBlk, mmv: &MMV) -> fmt::Result {
    writeln!(f, "TOC[{}]: toc offset {}, instances offset {} ({} entries)",
        instance_toc._toc_index(), instance_toc._mmv_offset(), instance_toc.sec_offset(), instance_toc.entries())?;

    for (offset, instance) in mmv.instance_blks() {
        write!(f, "  ")?;
        match *instance.indom_offset() {
            Some(ref indom_offset) => {
                let indom = mmv.indom_blks().get(indom_offset).unwrap();
                match *indom.indom() {
                    Some(ref indom_id) => write!(f, "[{}", indom_id)?,
                    None => write!(f, "[(no indom)")?
                }
            },
            None => write!(f, "[(no indom)")?
        }
        write!(f, "/{}] instance = [{} or \"", offset, instance.internal_id())?;
        write_version_specific_string(f, instance.external_id(), mmv)?;
        writeln!(f, "\"]")?;
    }

    Ok(())
}

fn write_metrics(f: &mut fmt::Formatter, metric_toc: &TocBlk, mmv: &MMV) -> fmt::Result {
    writeln!(f, "TOC[{}]: toc offset {}, metrics offset {} ({} entries)",
        metric_toc._toc_index(), metric_toc._mmv_offset(), metric_toc.sec_offset(), metric_toc.entries())?;

    for (offset, metric) in mmv.metric_blks() {
        if let Some(item) = *metric.item() {
            write!(f, "  [{}/{}] ", item, offset)?;
            write_version_specific_string(f, metric.name(), mmv)?;
            writeln!(f, "")?;

            write!(f, "      ")?;
            match MTCode::from_u32(metric.typ()) {
                Some(mtcode) => write!(f, "type={}", mtcode)?,
                None => write!(f, "(invalid type)")?
            }
            write!(f, ", ")?;
            match Semantics::from_u32(metric.sem()) {
                Some(sem) => write!(f, "sem={}", sem)?,
                None => write!(f, "(invalid semantics)")?
            }
            write!(f, ", ")?;
            writeln!(f, "pad=0x{:x}", metric.pad())?;
            
            writeln!(f, "      unit={}", Unit::from_raw(metric.unit()))?;

            write!(f, "      ")?;
            match *metric.indom() {
                Some(indom) => writeln!(f, "indom={}", indom)?,
                None => writeln!(f, "(no indom)")?
            }

            write!(f, "      ")?;
            match *metric.short_help_offset() {
                Some(ref short_help_offset) => {
                    let shortext = mmv.string_blks().get(short_help_offset).unwrap().string();
                    writeln!(f, "shorttext={}", shortext)?;
                }
                None => writeln!(f, "(no shorttext)")?
            }

            write!(f, "      ")?;
            match *metric.long_help_offset() {
                Some(ref long_help_offset) => {
                    let longtext = mmv.string_blks().get(long_help_offset).unwrap().string();
                    writeln!(f, "longtext={}", longtext)?;
                }
                None => writeln!(f, "(no longtext)")?
            }
        }
    }

    Ok(())
}

fn write_values(f: &mut fmt::Formatter, value_toc: &TocBlk, mmv: &MMV) -> fmt::Result {
    writeln!(f, "TOC[{}]: toc offset {}, values offset {} ({} entries)",
        value_toc._toc_index(), value_toc._mmv_offset(), value_toc.sec_offset(), value_toc.entries())?;

    for (offset, value) in mmv.value_blks() {
        if let Some(ref metric_offset) = *value.metric_offset() {
            let metric = mmv.metric_blks().get(&metric_offset).unwrap();
            if let Some(item) = *metric.item() {
                write!(f, "  [{}/{}] ", item, offset)?;
                write_version_specific_string(f, metric.name(), mmv)?;

                if let Some(ref instance_offset) = *value.instance_offset() {
                    let instance = mmv.instance_blks().get(&instance_offset).unwrap();
                    write!(f, "[{} or \"", instance.internal_id())?;
                    write_version_specific_string(f, instance.external_id(), mmv)?;
                    write!(f, "\"]")?;
                }

                write!(f, " = ")?;
                match *value.string_offset() {
                    Some(ref string_offset) => {
                        let string = mmv.string_blks().get(string_offset).unwrap();
                        writeln!(f, "\"{}\"", string.string())?;
                    }
                    None => {
                        match MTCode::from_u32(metric.typ()) {
                            Some(mtcode) => {
                                match mtcode {
                                    MTCode::U64 | MTCode::U32 => writeln!(f, "{}", value.value())?,
                                    MTCode::I64 => writeln!(f, "{}", value.value() as i64)?,
                                    MTCode::I32 => writeln!(f, "{}", value.value() as i32)?,
                                    MTCode::F32 => {
                                        let float = unsafe {
                                            mem::transmute::<u32, f32>(value.value() as u32)
                                        };
                                        writeln!(f, "{}", float)?
                                    },
                                    MTCode::F64 => {
                                        let double = unsafe {
                                            mem::transmute::<u64, f64>(value.value())
                                        };
                                        writeln!(f, "{}", double)?
                                    },
                                    MTCode::String => writeln!(f, "(no string offset)")?,
                                }
                            },
                            None => writeln!(f, "{}", value.value())?
                        }
                    },
                }
            }
        }
    }

    Ok(())
}

fn write_strings(f: &mut fmt::Formatter, string_toc: &TocBlk, mmv: &MMV) -> fmt::Result {
    writeln!(f, "TOC[{}]: toc offset {}, strings offset {} ({} entries)",
        string_toc._toc_index(), string_toc._mmv_offset(), string_toc.sec_offset(), string_toc.entries())?;

    for (i, (offset, string)) in mmv.string_blks().iter().enumerate() {
        writeln!(f, "  [{}/{}] {}", i+1, offset, string.string())?;
    }

    Ok(())
}

impl fmt::Display for MMV {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        writeln!(f, "{}", self.header)?;

        if let Some(ref indom_toc) = self.indom_toc {
            write_indoms(f, indom_toc, self)?;
            writeln!(f, "")?;
        }

        if let Some(ref instance_toc) = self.instance_toc {
            write_instances(f, instance_toc, self)?;
            writeln!(f, "")?;
        }

        write_metrics(f, &self.metric_toc, self)?;
        writeln!(f, "")?;

        write_values(f, &self.value_toc, self)?;
        writeln!(f, "")?;

        if let Some(ref string_toc) = self.string_toc {
            write_strings(f, string_toc, self)?;
            writeln!(f, "")?;
        }

        Ok(())
    }
}
