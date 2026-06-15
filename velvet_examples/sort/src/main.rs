use velvet::prelude::*;
use std::{env, time::Instant};
use rand::{SeedableRng, rngs::StdRng, distr::{Distribution, Uniform}};
include!(concat!(env!("OUT_DIR"), "/velvet_app.rs"));

mod par_merge;

pub(crate) const DIRECT_THRESHOLD: usize = 256 * 1024;

fn gen_vec(n: usize, seed: usize) -> Vec<i32> {
    let range: Uniform<i32> = Uniform::try_from(i32::MIN..i32::MAX).unwrap();
    let mut rng = StdRng::seed_from_u64(seed as u64); 

    (0..n).map(|_| range.sample(&mut rng)).collect()
}
fn _check(sorted: Vec<i32>) {
    eprintln!("checking vec size: {}", sorted.len());
    // println!("sorted = {:?}", sorted);
    let mut prev = sorted[0];
    for i in 1..sorted.len() {
        if prev == 0 && sorted[i] == 0 {
            eprintln!("DOUBLE ZEROES!! at idx {}", i);
            return;
        }
        if sorted[i] < prev {
            eprintln!("NOT SORTED! discovered at idx = {}", i);
            return;
        } else {
            prev = sorted[i];
        }
    }
    eprintln!("SORTED!");
}

// sort src in-place, using buf if split is necessary
fn merge_sort(src: &mut [i32], buf: &mut [i32]) {
    if src.len() <= DIRECT_THRESHOLD {
        src.sort();
        return;
    }

    // split and sort (using buffer)
    let mid = src.len() / 2;
    let (left_src, right_src) = src.split_at_mut(mid);
    let (left_buf, right_buf) = buf.split_at_mut(mid);
    let (left_sorted, right_sorted) = (sort_into(left_src, left_buf), sort_into(right_src, right_buf));

    // merge buffers into src
    merge(left_sorted, right_sorted, src);
}

// sort src into dest
fn sort_into<'dest>(src: &mut [i32], dest: &'dest mut [i32]) -> &'dest [i32] {
    let mid = src.len() / 2;
    let (left_src, right_src) = src.split_at_mut(mid);
    
    // sort each half
    let (left_dest, right_dest) = dest.split_at_mut(mid);
    merge_sort(left_src, left_dest);
    merge_sort(right_src, right_dest);
    

    // merge the sorted halves into dest
    merge(left_src, right_src, dest);
    dest
}

fn merge(left: &[i32], right: &[i32], dest: &mut [i32]) {
    let max = Ord::max(*left.last().unwrap(), *right.last().unwrap());
    let mut left = left.iter();
    let mut left_n = *left.next().unwrap();
    let mut right = right.iter();
    let mut right_n = *right.next().unwrap();
    for d in dest.iter_mut() {
        if left_n < right_n {
            *d = left_n;
            left_n = match left.next() {
                Some(val) => *val,
                None => max,
            }
        } else {
            *d = right_n;
            right_n = match right.next() {
                Some(val) => *val,
                None => max,
            }
        }
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 4 {
        println!("usage for mergesort: cargo run [cargo_options] [velvet|seq|rayon] [vec size] [random seed]");
        println!("example: cargo run --release velvet 100000 42");
        return;
    }

    let app = &args[1];
    let n: usize = args[2].parse().unwrap();
    let seed: usize = args[3].parse().unwrap();

    let arr = gen_vec(n, seed);

    if app.eq("velvet") {
        velvet_main(arr, seed);
    } else if app.eq("seq") {
        seq_main(arr, seed);
    } else {
        eprint!("Unrecognized app: {}", app);
    }
}

fn seq_main(mut arr: Vec<i32>, seed: usize){
    let n: usize = arr.len();
    let mut buf: Vec<i32> = (0..n).map(|_| 0).collect();
    let start = Instant::now();
    merge_sort(&mut arr, &mut buf);
    let end = start.elapsed();

    println!("0,1,0,{},{},{}", n, seed, end.as_secs_f32());
    _check(arr);
}

#[velvet_main]
fn velvet_main(arr: Vec<i32>, seed: usize) {
    let len = arr.len();

    use std::sync::atomic;
    par_merge::VEC.set(arr).unwrap();
    let vec: Vec<atomic::AtomicIsize> = (0..len).map(|_| atomic::AtomicIsize::new(0)).collect();
    par_merge::VEC_SORTED.set(vec).unwrap();
    let vec: Vec<atomic::AtomicIsize> = (0..len).map(|_| atomic::AtomicIsize::new(0)).collect();
    par_merge::VEC_MERGED.set(vec).unwrap();

    let start = Instant::now();
    par_merge::sort_spawn(0, len, true);
    let end = start.elapsed();
    
    let version = match velvet_get_queue_name().as_str() {
        "safe" => 2,
        "unsafe" => 20,
        "crossbeam" => 21,
        _ => -10,
    };
    println!("{},{},{},{},{},{}", version, velvet_get_num_workers(), DIRECT_THRESHOLD, len, seed, end.as_secs_f32());
    let sorted: Vec<i32> = par_merge::VEC_SORTED.get().unwrap().iter().map(|x| x.load(std::sync::atomic::Ordering::Relaxed) as i32).collect();
    _check(sorted);
}