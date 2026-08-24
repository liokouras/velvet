// mergesort w parallel sort and parallel merge using global buffers
use velvet::prelude::*;
use std::sync::{atomic::{AtomicIsize, Ordering}};

const SORT_CHUNK: usize = super::DIRECT_THRESHOLD;
const MERGE_CHUNK: usize = 2*super::DIRECT_THRESHOLD;

#[spawnable(sync_hint=["merge_spawn"])]
pub(super) fn sort_spawn(sorted: &'static [AtomicIsize], merged: &'static [AtomicIsize], vec: &'static [i32], usebuf: bool) {
    let len = sorted.len();
    let mid = len / 2;

    if usebuf && len < SORT_CHUNK {
        let mut seq_vec = Vec::from(&vec[0..len]);
        seq_vec.sort();
        for (idx, i) in seq_vec.iter().enumerate() {
            sorted[idx].store(*i as isize, Ordering::Relaxed);
        }
        return;
    }
    
    sort_spawn(&sorted[..mid], &merged[..mid], &vec[..mid], !usebuf);
    sort_spawn(&sorted[mid..], &merged[mid..], &vec[mid..],  !usebuf);

    if usebuf {
        merge_spawn(&merged[..mid], &merged[mid..], sorted);
    } else {
        merge_spawn(&sorted[..mid], &sorted[mid..], merged);
    }

}

#[spawnable]
pub(super) fn merge_spawn(mut left: &'static [AtomicIsize], mut right: &'static [AtomicIsize], dest: &'static [AtomicIsize]){
    if dest.len() <= MERGE_CHUNK {
        merge_seq(left, right, dest);
        return;
    }

    // let 'left' be the larger of the two sub-arrays
    if left.len() < right.len() {
        let tmp_left = left;
        left = right;
        right = tmp_left;
    };

    // find the middle element of left, and use binary_search to find suitable index in right
    let a_mid = left.len() / 2;
    let val = left[a_mid].load(Ordering::Relaxed);
    let b_mid = binary_search(right, val, 0, right.len());

    // recurse, splitting at the newly found 'mid' indexes (in the sense of value, not array size)
    merge_spawn(&left[..a_mid], &right[..b_mid], &dest[..a_mid+b_mid]);
    merge_spawn(&left[a_mid..], &right[b_mid..], &dest[a_mid+b_mid..]);
}

fn merge_seq (left: &'static [AtomicIsize], right: &'static [AtomicIsize], dest: &'static [AtomicIsize]){
    let left_len = left.len();
    let right_len = right.len();

    if right_len <= 0 {
        for (idx, i) in left.into_iter().enumerate() {
            dest[idx].store(i.load(Ordering::Relaxed), Ordering::Relaxed);
        }
        return;
    }

    let max = Ord::max(right[right_len-1].load(Ordering::Relaxed), left[left_len-1].load(Ordering::Relaxed));
    let mut left_i = (0..left.len()).into_iter();
    let mut left_n = left[0].load(Ordering::Relaxed);
    let mut right_i = (0..right.len()).into_iter();
    let mut right_n = right[0].load(Ordering::Relaxed);
    for d in 0..dest.len() {
        if left_n < right_n {
            dest[d].store(left_n, Ordering::Relaxed);
            left_n = match left_i.next() {
                Some(val) => left[val].load(Ordering::Relaxed),
                None => max,
            }
        } else {
            dest[d].store(right_n, Ordering::Relaxed);
            right_n = match right_i.next() {
                Some(val) => right[val].load(Ordering::Relaxed),
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