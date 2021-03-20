#![feature(test)]

extern crate test;

use jujube_lib::diff;
use test::Bencher;

fn unchanged_lines(count: usize) -> (String, String) {
    let mut lines = vec![];
    for i in 0..count {
        lines.push(format!("left line {}\n", i));
    }
    (lines.join(""), lines.join(""))
}

fn modified_lines(count: usize) -> (String, String) {
    let mut left_lines = vec![];
    let mut right_lines = vec![];
    for i in 0..count {
        left_lines.push(format!("left line {}\n", i));
        right_lines.push(format!("right line {}\n", i));
    }
    (left_lines.join(""), right_lines.join(""))
}

fn reversed_lines(count: usize) -> (String, String) {
    let mut left_lines = vec![];
    for i in 0..count {
        left_lines.push(format!("left line {}\n", i));
    }
    let mut right_lines = left_lines.clone();
    right_lines.reverse();
    (left_lines.join(""), right_lines.join(""))
}

#[bench]
fn bench_diff_1k_unchanged_lines(b: &mut Bencher) {
    let (left, right) = unchanged_lines(1000);
    b.iter(|| diff::diff(left.as_bytes(), right.as_bytes()));
}

#[bench]
fn bench_diff_10k_unchanged_lines(b: &mut Bencher) {
    let (left, right) = unchanged_lines(10000);
    b.iter(|| diff::diff(left.as_bytes(), right.as_bytes()));
}

#[bench]
fn bench_diff_1k_modified_lines(b: &mut Bencher) {
    let (left, right) = modified_lines(1000);
    b.iter(|| diff::diff(left.as_bytes(), right.as_bytes()));
}

#[bench]
fn bench_diff_10k_modified_lines(b: &mut Bencher) {
    let (left, right) = modified_lines(10000);
    b.iter(|| diff::diff(left.as_bytes(), right.as_bytes()));
}

#[bench]
fn bench_diff_1k_lines_reversed(b: &mut Bencher) {
    let (left, right) = reversed_lines(1000);
    b.iter(|| diff::diff(left.as_bytes(), right.as_bytes()));
}

#[bench]
fn bench_diff_10k_lines_reversed(b: &mut Bencher) {
    let (left, right) = reversed_lines(10000);
    b.iter(|| diff::diff(left.as_bytes(), right.as_bytes()));
}
