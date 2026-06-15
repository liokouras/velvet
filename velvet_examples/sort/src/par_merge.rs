// mergesort w parallel sort and parallel merge using global buffers
use velvet::prelude::*;
use std::sync::{atomic::{AtomicIsize, Ordering}, OnceLock};

const SORT_CHUNK: usize = super::DIRECT_THRESHOLD;
const MERGE_CHUNK: usize = 2*super::DIRECT_THRESHOLD;

pub(super) static VEC:OnceLock<Vec<i32>> = OnceLock::new();
pub(super) static VEC_SORTED:OnceLock<Vec<AtomicIsize>> = OnceLock::new();
pub(super) static VEC_MERGED:OnceLock<Vec<AtomicIsize>> = OnceLock::new();

#[spawnable(sync_hint=["merge_spawn"])]
pub(super) fn sort_spawn(left: usize, right: usize, usebuf: bool) {
    let len = right-left;
    let mid = len / 2;

    if usebuf && len < SORT_CHUNK {
        let vec = VEC.get().unwrap();
        let sorted_vec = VEC_SORTED.get().unwrap();
        let mut seq_vec = Vec::from(&vec[left..right]);
        seq_vec.sort();
        for (idx, i) in (left..right).enumerate() {
            sorted_vec[i].store(seq_vec[idx] as isize, Ordering::Relaxed);
        }
        return;
    }
    
    sort_spawn(left, left+mid, !usebuf);
    sort_spawn(left+mid, right, !usebuf);
    merge_spawn((left, left+mid), (left+mid, right), (left, right), usebuf);
}

#[spawnable]
pub(super) fn merge_spawn((mut a_left, mut a_right): (usize, usize), (mut b_left, mut b_right): (usize, usize), dest: (usize, usize), invec: bool) {
    if dest.1 - dest.0 <= MERGE_CHUNK {
        merge_seq((a_left, a_right), (b_left, b_right), dest, invec);
        return;
    }

    let (src_left, src_right) = if invec {
        (&VEC_MERGED.get().unwrap().as_slice()[a_left..a_right],
        &VEC_MERGED.get().unwrap().as_slice()[b_left..b_right])
    } else {
        (&VEC_SORTED.get().unwrap().as_slice()[a_left..a_right],
        &VEC_SORTED.get().unwrap().as_slice()[b_left..b_right])
    };

    // let 'left' be the larger of the two sub-arrays
    let (left, right) = if src_left.len() > src_right.len() { (src_left, src_right) }
    else { 
        let (tmp_left, tmp_right) = (a_left, a_right);
        (a_left, a_right) = (b_left, b_right);
        (b_left, b_right) = (tmp_left, tmp_right);
        (src_right, src_left)
    };

    // find the middle element of left, and use std's binary_search to find suitable index in right
    let a_mid = left.len() / 2;
    let val = left[a_mid].load(Ordering::Relaxed);
    let b_mid = binary_search(right, val, 0, right.len());

    // recurse, splitting at the newly found 'mid' indexes (in the sense of value, not array size)
    merge_spawn((a_left, a_left + a_mid), (b_left, b_left+b_mid), (dest.0, dest.0+a_mid+b_mid), invec);
    merge_spawn((a_left + a_mid, a_right), (b_left + b_mid, b_right), (dest.0+a_mid+b_mid, dest.1), invec);
}

fn merge_seq ((a_left, a_right): (usize, usize), (b_left, b_right): (usize, usize), (d_left, d_right): (usize, usize), invec: bool) {
    let (src, dest) = if invec {
        (VEC_MERGED.get().unwrap().as_slice(),
        VEC_SORTED.get().unwrap().as_slice())
    } else {
        (VEC_SORTED.get().unwrap().as_slice(),
        VEC_MERGED.get().unwrap().as_slice())
    };

    if b_right <= b_left {
        for (idx, i) in (d_left .. d_right).enumerate() {
            dest[i].store(src[a_left+idx].load(Ordering::Relaxed), Ordering::Relaxed);
        }
        return;
    }

    let max = Ord::max(src[a_right-1].load(Ordering::Relaxed), src[b_right-1].load(Ordering::Relaxed));
    let mut left = (a_left+1..a_right).into_iter();
    let mut left_n = src[a_left].load(Ordering::Relaxed);
    let mut right = (b_left+1..b_right).into_iter();
    let mut right_n = src[b_left].load(Ordering::Relaxed);
    for d in d_left .. d_right{
        if left_n < right_n {
            dest[d].store(left_n, Ordering::Relaxed);
            left_n = match left.next() {
                Some(val) => src[val].load(Ordering::Relaxed),
                None => max,
            }
        } else {
            dest[d].store(right_n, Ordering::Relaxed);
            right_n = match right.next() {
                Some(val) => src[val].load(Ordering::Relaxed),
                None => max,
            }
        }
    }
}

fn binary_search(src: &[AtomicIsize], target: isize, mut b_left: usize, mut b_right: usize) -> usize {
    while b_left < b_right {
        let b_mid = b_left + (b_right - b_left) / 2;
        let mid_val = src[b_mid].load(Ordering::Relaxed);

        if mid_val == target { return b_mid; }
        else if mid_val < target { b_left = b_mid + 1; }
        else { b_right = b_mid; }
    }
    b_left
}