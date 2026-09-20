// FFT, assumes N is a power of 2
mod fft;
use fft::Complex64;

use std::sync::Mutex;

fn build_twiddles(n: usize) -> Vec<Complex64> {
    let two_pi_over_n = 2.0 * std::f64::consts::PI / n as f64;
    (0..=n)
    .map(|k| {
        let theta = two_pi_over_n * k as f64;
        Complex64::new(theta.cos(), -theta.sin())
    })
    .collect()
}

fn main() {
    let mut x = Vec::new();
    let mut y= Vec::new();

    for _ in 0..64 {
        let mut tmp = Vec::with_capacity(128);
        for i in 0..128 {
            tmp.push(Complex64::new(i as f64, 0 as f64));
        }
        x.push(Mutex::new(tmp.clone()));
        y.push(Mutex::new(tmp));
    }
    let n = x.len()*128;
    let inp = Box::leak(Box::new(x));
    let out = Box::leak(Box::new(y));
    let twiddles = build_twiddles(n);
    let twiddles = Box::leak(Box::new(twiddles));
    let _res = fft::fft(&[0,1,2], inp, out, twiddles);
}