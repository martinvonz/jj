use bencher::{benchmark_group, benchmark_main, Bencher};
use criterion_bencher_compat as bencher;
use itertools::Itertools;
use jujutsu_lib::repo::Trie;

fn new_change_id() -> Vec<u8> {
    uuid::Uuid::new_v4().as_bytes().to_vec()
}

fn insert(trie: &mut Trie<u8, Vec<u8>>, count: usize) {
    for _i in 0..count {
        let id = new_change_id();
        trie.insert(&id.clone(), id);
        // trie.insert(&id, vec![]); // This checks iteration speed
    }
}

fn insertion_test(b: &mut Bencher, n: usize) {
    b.iter(|| {
        let mut trie = Trie::new();
        insert(&mut trie, n)
    });
}

fn iteration_test(b: &mut Bencher, n: usize) {
    let mut trie = Trie::new();
    insert(&mut trie, n);
    b.iter(|| trie.itervalues().count());
}

fn prefix_test(b: &mut Bencher, n: usize) {
    let mut trie = Trie::new();
    insert(&mut trie, n);
    let v = trie.itervalues().collect_vec();
    b.iter(|| {
        v.iter()
            .map(|value| trie.shortest_unique_prefix_len(value))
            .max()
    });
}

fn trie_10k_insertions(b: &mut Bencher) {
    insertion_test(b, 10000)
}

fn trie_10k_prefixes(b: &mut Bencher) {
    prefix_test(b, 10000)
}

fn trie_20k_insertions(b: &mut Bencher) {
    insertion_test(b, 20000)
}

fn trie_20k_prefixes(b: &mut Bencher) {
    prefix_test(b, 20000)
}

fn trie_20k_iterations(b: &mut Bencher) {
    iteration_test(b, 20000)
}

fn trie_50k_insertions(b: &mut Bencher) {
    insertion_test(b, 50000)
}

fn trie_200k_insertions(b: &mut Bencher) {
    insertion_test(b, 200000)
}

benchmark_group!(
    trie_benches,
    trie_20k_iterations,
    trie_10k_insertions,
    trie_20k_insertions,
    trie_50k_insertions,
    trie_200k_insertions,
    trie_10k_prefixes,
    trie_20k_prefixes,
);
benchmark_main!(trie_benches);
