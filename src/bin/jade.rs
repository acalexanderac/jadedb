use jadedb::{Db, FsyncMode, Options};
use std::env;
use std::path::PathBuf;
use std::time::Instant;

fn main() {
    let mut args = env::args().skip(1);
    let cmd = args.next().unwrap_or_else(|| "help".into());
    match cmd.as_str() {
        "bench" => bench(args.next()),
        "put" => {
            let dir = args.next().expect("dir");
            let key = args.next().expect("key");
            let val = args.next().unwrap_or_default();
            let db = Db::open(PathBuf::from(dir), Options::default()).expect("open");
            db.put(key.as_bytes(), val.as_bytes()).expect("put");
            db.sync().expect("sync");
            println!("ok");
        }
        "get" => {
            let dir = args.next().expect("dir");
            let key = args.next().expect("key");
            let db = Db::open(PathBuf::from(dir), Options::default()).expect("open");
            match db.get(key.as_bytes()).expect("get") {
                Some(v) => println!("{}", String::from_utf8_lossy(&v)),
                None => println!("(null)"),
            }
        }
        _ => {
            eprintln!("jade put <dir> <key> [value]");
            eprintln!("jade get <dir> <key>");
            eprintln!("jade bench [always|batch|never]");
        }
    }
}

fn bench(mode: Option<String>) {
    let fsync = match mode.as_deref() {
        Some("always") => FsyncMode::Always,
        Some("never") => FsyncMode::Never,
        _ => FsyncMode::Batch,
    };
    let dir = tempfile::tempdir().expect("tmpdir");
    let mut opts = Options::for_tests();
    opts.fsync = fsync;
    opts.memtable_bytes = 256 * 1024;
    let db = Db::open(dir.path(), opts).expect("open");
    let n = 10_000;
    let start = Instant::now();
    for i in 0..n {
        let k = format!("key-{i:08}");
        let v = format!("val-{i:08}");
        db.put(k.as_bytes(), v.as_bytes()).expect("put");
    }
    db.flush().expect("flush");
    let elapsed = start.elapsed();
    let stats = db.stats();
    println!("fsync={fsync:?} n={n} elapsed={elapsed:?}");
    println!(
        "ops_per_sec={:.0} write_amp={:.2} stalls={} flushes={} compacts={} l0={}",
        n as f64 / elapsed.as_secs_f64(),
        stats.write_amplification,
        stats.stall_count,
        stats.flush_count,
        stats.compact_count,
        stats.l0_files
    );
}
