extern crate hornet;
extern crate enum_primitive;

use hornet::mmv::*;
use hornet::client::metric::Semantics;
use enum_primitive::FromPrimitive;
use std::env;
use std::path::Path;

fn print_header(mmv: &MMV) {
    let hdr = &mmv.header;
    println!("Version    = {}", hdr.version);
    println!("Generated  = {}", hdr.gen1);
    println!("TOC count  = {}", hdr.toc_count);
    println!("Cluster    = {}", hdr.cluster_id);
    println!("Process    = {}", hdr.pid);
    println!("Flags      = 0x{:x}", hdr.flags);
}

fn print_indoms(mmv: &MMV, toc_index: u8) -> bool {
    if let Some(ref indom_toc) = mmv.indom_toc {
        println!("TOC[{}]: toc offset {}, indoms offset {} ({} entries)",
            toc_index, indom_toc._mmv_offset, indom_toc.sec_offset, indom_toc.entries);

        for (offset, indom) in &mmv.indom_blks {
            if let Some(ref indom_id) = indom.indom {
                print!("  [{}/{}] {} instances, starting at offset ",
                    indom_id, offset, indom.instances);
                if let Some(instances_offset) = indom.instances_offset {
                    println!("{}", instances_offset);
                } else {
                    println!("(no instances)")
                }
        
                if let Some(ref short_help_offset) = indom.short_help_offset {
                    let shortext = &mmv.string_blks.get(short_help_offset).unwrap().string;
                    println!("      shorttext={}", shortext);
                } else {
                    println!("      (no shorttext)");
                }

                if let Some(ref long_help_offset) = indom.long_help_offset {
                    let longtext = &mmv.string_blks.get(long_help_offset).unwrap().string;
                    println!("      longtext={}", longtext);
                } else {
                    println!("      (no longtext)");
                }
            }
        }
        true
    } else {
        false
    }
}

fn print_instances(mmv: &MMV, toc_index: u8) -> bool {
    if let Some(ref instance_toc) = mmv.instance_toc {
        println!("TOC[{}]: toc offset {}, instances offset {} ({} entries)",
            toc_index, instance_toc._mmv_offset, instance_toc.sec_offset, instance_toc.entries);

        for (offset, instance) in &mmv.instance_blks {
            if let Some(ref indom_offset) = instance.indom_offset {
                let indom = &mmv.indom_blks.get(indom_offset).unwrap();
                if let Some(ref indom_id) = indom.indom {
                    print!("  [{}", indom_id);
                } else {
                    print!("  [(no indom)");
                }
            } else {
                print!("  [(no indom)");
            }
            
            println!("{} instance = [{} or \"{}\"]", offset, instance.internal_id, instance.external_id);
        }
        true
    } else {
        false
    }
}

fn print_metrics(mmv: &MMV, toc_index: u8) {
    let metric_toc = &mmv.metric_toc;
    println!("TOC[{}]: toc offset {}, metrics offset {} ({} entries)",
        toc_index, metric_toc._mmv_offset, metric_toc.sec_offset, metric_toc.entries);

    for (offset, metric) in &mmv.metric_blks {
        if let Some(item) = metric.item {
            println!("  [{}/{}] {}", item, offset, metric.name);

            print!("      ");
            match MTCode::from_u32(metric.typ) {
                Some(mtcode) => print!("type={}", mtcode),
                None => print!("(invalid type)")
            }
            print!(", ");
            match Semantics::from_u32(metric.sem) {
                Some(sem) => print!("sem={}", sem),
                None => print!("(invalid semantics)")
            }
            print!(", ");
            println!("pad=0x{:x}", metric.pad);
            
            println!("      unit={}", metric.unit);

            if let Some(indom) = metric.indom {
                println!("      indom={}", indom);
            } else {
                println!("      (no indom)");
            }

            if let Some(ref short_help_offset) = metric.short_help_offset {
                let shortext = &mmv.string_blks.get(short_help_offset).unwrap().string;
                println!("      shorttext={}", shortext);
            } else {
                println!("      (no shorttext)");
            }

            if let Some(ref long_help_offset) = metric.long_help_offset {
                let longtext = &mmv.string_blks.get(long_help_offset).unwrap().string;
                println!("      longtext={}", longtext);
            } else {
                println!("      (no longtext)");
            }
        }
    }
}

fn print_values(mmv: &MMV, toc_index: u8) {
    let value_toc = &mmv.value_toc;
    println!("TOC[{}]: toc offset {}, values offset {} ({} entries)",
        toc_index, value_toc._mmv_offset, value_toc.sec_offset, value_toc.entries);

    for (offset, value) in &mmv.value_blks {
        if let Some(ref metric_offset) = value.metric_offset {
            let metric = mmv.metric_blks.get(&metric_offset).unwrap();
            if let Some(item) = metric.item {
                print!("  [{}/{}] {}", item, offset, metric.name);

                if let Some(ref instance_offset) = value.instance_offset {
                    let instance = mmv.instance_blks.get(&instance_offset).unwrap();
                    print!("[{} or \"{}\"]", instance.internal_id, instance.external_id);
                }

                if let Some(ref string_offset) = value.string_offset {
                    let string = mmv.string_blks.get(&string_offset).unwrap();
                    println!(" = \"{}\"", string.string);
                } else {
                    println!(" = {}", value.value);
                }
            }
        }
    }
}

fn print_strings(mmv: &MMV, toc_index: u8) {
    if let Some(ref string_toc) = mmv.string_toc {
        println!("TOC[{}]: toc offset {}, strings offset {} ({} entries)",
            toc_index, string_toc._mmv_offset, string_toc.sec_offset, string_toc.entries);

        for (i, (offset, string)) in mmv.string_blks.iter().enumerate() {
            println!("  [{}/{}] {}", i+1, offset, string.string);
        }
    }
}

fn main() {
    let path_arg = env::args().nth(1)
        .expect("Specify path to mmv file");
    let mmv_path = Path::new(&path_arg);

    let mmv = dump(&mmv_path).unwrap();

    print_header(&mmv);
    println!("");

    let mut toc_index = 0;

    if print_indoms(&mmv, toc_index) {
        println!(" ");
        toc_index += 1;
    }

    if print_instances(&mmv, toc_index) {
        println!(" ");
        toc_index += 1;
    }

    print_metrics(&mmv, toc_index);
    println!(" ");
    toc_index += 1;

    print_values(&mmv, toc_index);
    println!(" ");
    toc_index += 1;

    print_strings(&mmv, toc_index);
    println!(" ");
}
