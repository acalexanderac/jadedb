use criterion::{criterion_group, criterion_main, Criterion};
use jadedb::{Db, FsyncMode, Options};
use std::hint::black_box;

fn write_bench(c: &mut Criterion) {
    let mut group = c.benchmark_group("put");
    for mode in [FsyncMode::Never, FsyncMode::Batch, FsyncMode::Always] {
        group.bench_function(format!("{mode:?}"), |b| {
            b.iter_with_setup(
                || {
                    let dir = tempfile::tempdir().unwrap();
                    let mut opts = Options::for_tests();
                    opts.fsync = mode;
                    opts.memtable_bytes = 1024 * 1024;
                    let path = dir.path().to_path_buf();
                    let db = Db::open(&path, opts).unwrap();
                    (dir, db)
                },
                |(_dir, db)| {
                    for i in 0..200 {
                        let k = format!("k{i}");
                        db.put(black_box(k.as_bytes()), black_box(b"v")).unwrap();
                    }
                },
            );
        });
    }
    group.finish();
}

criterion_group!(benches, write_bench);
criterion_main!(benches);
