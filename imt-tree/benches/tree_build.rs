use criterion::{criterion_group, criterion_main, Criterion};
use ff::Field;
use pasta_curves::Fp;

use imt_tree::tree::{
    build_levels, build_punctured_ranges, commit_punctured_ranges, precompute_empty_hashes,
    TREE_DEPTH,
};

fn bench_punctured_tree_build(c: &mut Criterion) {
    let mut rng = rand::thread_rng();

    let step = Fp::from(2u64).pow([250, 0, 0, 0]);
    let mut nfs: Vec<Fp> = (0u64..=16).map(|k| step * Fp::from(k)).collect();
    nfs.push(Fp::one().neg());
    let extra: Vec<Fp> = (0..100_000).map(|_| Fp::random(&mut rng)).collect();
    nfs.extend(extra);
    nfs.sort();
    nfs.dedup();
    if nfs.len() % 2 == 0 {
        nfs.insert(1, Fp::one());
    }

    c.bench_function("punctured_tree_build_100k", |b| {
        b.iter(|| {
            let ranges = build_punctured_ranges(&nfs);
            let leaves = commit_punctured_ranges(&ranges);
            let empty = precompute_empty_hashes();
            let _ = build_levels(leaves, &empty, TREE_DEPTH);
        });
    });
}

criterion_group! {
    name = benches;
    config = Criterion::default().sample_size(10);
    targets = bench_punctured_tree_build
}
criterion_main!(benches);
