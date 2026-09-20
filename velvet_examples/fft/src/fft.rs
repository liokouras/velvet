use std::{sync::Mutex, todo};
use std::f64::consts::FRAC_1_SQRT_2;

/// Minimal complex type
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Complex64 {
    pub re: f64,
    pub im: f64,
}
impl Complex64 {
    #[inline]
    pub fn new(re: f64, im: f64) -> Self {
        Complex64 { re, im }
    }
    #[inline]
    pub fn zero() -> Self {
        Complex64 { re: 0.0, im: 0.0 }
    }
}
impl std::ops::Add for Complex64 {
    type Output = Complex64;
    #[inline]
    fn add(self, o: Complex64) -> Complex64 {
        Complex64::new(self.re + o.re, self.im + o.im)
    }
}
impl std::ops::Sub for Complex64 {
    type Output = Complex64;
    #[inline]
    fn sub(self, o: Complex64) -> Complex64 {
        Complex64::new(self.re - o.re, self.im - o.im)
    }
}
impl std::ops::Mul for Complex64 {
    type Output = Complex64;
    #[inline]
    fn mul(self, o: Complex64) -> Complex64 {
        Complex64::new(self.re * o.re - self.im * o.im, self.re * o.im + self.im * o.re)
    }
}

const CHUNK_SIZE: usize = 128;
pub type FftBuf = [Mutex<Vec<Complex64>>];

//out[a + i] = in[(a+i)*2 + 0] ; out[a + i + m]   = in[(a+i)*2 + 1]
fn unshuffle2(a: usize, inp: &'static FftBuf, out: &'static FftBuf, m: usize) {
    debug_assert_eq!(a % CHUNK_SIZE, 0);
    let in_chunk0 = (a * 2) / CHUNK_SIZE;
    let inp0 = inp[in_chunk0].lock().unwrap();
    let inp1 = inp[in_chunk0 + 1].lock().unwrap();
    let mut out0 = out[a / CHUNK_SIZE].lock().unwrap();
    let mut out1 = out[(a + m) / CHUNK_SIZE].lock().unwrap();

    let mut ip = 0usize;
    for i in 0..CHUNK_SIZE/2 {
        out0[i] = inp0[ip];
        out1[i] = inp0[ip + 1];
        ip += 2;
    }
    ip = 0usize;
    for i in CHUNK_SIZE/2..CHUNK_SIZE {
        out0[i] = inp1[ip];
        out1[i] = inp1[ip + 1];
        ip += 2;
    }
}
// out[a+i+p*m] = in[(a+i)*4 + p], p in 0..4
fn unshuffle4(a: usize, inp: &'static FftBuf, out: &'static FftBuf, m: usize){
    debug_assert_eq!(a % CHUNK_SIZE, 0);

    let in_chunk0 = (a * 4) / CHUNK_SIZE;
    let inp0 = inp[in_chunk0].lock().unwrap();
    let inp1 = inp[in_chunk0 + 1].lock().unwrap();
    let inp2 = inp[in_chunk0 + 2].lock().unwrap();
    let inp3 = inp[in_chunk0 + 3].lock().unwrap();

    let mut out0 = out[a / CHUNK_SIZE].lock().unwrap();
    let mut out1 = out[(a + m) / CHUNK_SIZE].lock().unwrap();
    let mut out2 = out[(a + 2*m) / CHUNK_SIZE].lock().unwrap();
    let mut out3 = out[(a + 3*m) / CHUNK_SIZE].lock().unwrap();

    let mut ip = 0usize;
    for i in 0..CHUNK_SIZE/4 {
        out0[i] = inp0[ip];
        out1[i] = inp0[ip + 1];
        ip += 2;
        out2[i] = inp0[ip];
        out3[i] = inp0[ip + 1];
        ip += 2;
    }
    ip = 0usize;
    for i in CHUNK_SIZE/4..CHUNK_SIZE/2 {
        out0[i] = inp1[ip];
        out1[i] = inp1[ip + 1];
        ip += 2;
        out2[i] = inp1[ip];
        out3[i] = inp1[ip + 1];
        ip += 2;
    }
    ip = 0usize;
    for i in CHUNK_SIZE/2..(3*CHUNK_SIZE/4) {
        out0[i] = inp2[ip];
        out1[i] = inp2[ip + 1];
        ip += 2;
        out2[i] = inp2[ip];
        out3[i] = inp2[ip + 1];
        ip += 2;
    }
    ip = 0usize;
    for i in (3*CHUNK_SIZE/4)..CHUNK_SIZE {
        out0[i] = inp3[ip];
        out1[i] = inp3[ip + 1];
        ip += 2;
        out2[i] = inp3[ip];
        out3[i] = inp3[ip + 1];
        ip += 2;
    }
}
// out[a+i+p*m] = in[(a+i)*8 + p], p in 0..8
fn unshuffle8(a: usize, inp: &'static FftBuf, out: &'static FftBuf, m: usize){
    debug_assert_eq!(a % CHUNK_SIZE, 0);

    let in_chunk0 = (a * 8) / CHUNK_SIZE;
    let inp0 = inp[in_chunk0].lock().unwrap();
    let inp1 = inp[in_chunk0 + 1].lock().unwrap();
    let inp2 = inp[in_chunk0 + 2].lock().unwrap();
    let inp3 = inp[in_chunk0 + 3].lock().unwrap();
    let inp4 = inp[in_chunk0 + 4].lock().unwrap();
    let inp5 = inp[in_chunk0 + 5].lock().unwrap();
    let inp6 = inp[in_chunk0 + 6].lock().unwrap();
    let inp7 = inp[in_chunk0 + 7].lock().unwrap();

    let mut out0 = out[a / CHUNK_SIZE].lock().unwrap();
    let mut out1 = out[(a + m) / CHUNK_SIZE].lock().unwrap();
    let mut out2 = out[(a + 2*m) / CHUNK_SIZE].lock().unwrap();
    let mut out3 = out[(a + 3*m) / CHUNK_SIZE].lock().unwrap();
    let mut out4 = out[(a + 4*m) / CHUNK_SIZE].lock().unwrap();
    let mut out5 = out[(a + 5*m) / CHUNK_SIZE].lock().unwrap();
    let mut out6 = out[(a + 6*m) / CHUNK_SIZE].lock().unwrap();
    let mut out7 = out[(a + 7*m) / CHUNK_SIZE].lock().unwrap();


    let mut ip = 0usize;
    for i in 0..CHUNK_SIZE/8 {
        out0[i] = inp0[ip];
        out1[i] = inp0[ip + 1];
        ip += 2;
        out2[i] = inp0[ip];
        out3[i] = inp0[ip + 1];
        ip += 2;
        out4[i] = inp0[ip];
        out5[i] = inp0[ip + 1];
        ip += 2;
        out6[i] = inp0[ip];
        out7[i] = inp0[ip + 1];
        ip += 2;
    }
    ip = 0usize;
    for i in CHUNK_SIZE/8..CHUNK_SIZE/4 {
        out0[i] = inp1[ip];
        out1[i] = inp1[ip + 1];
        ip += 2;
        out2[i] = inp1[ip];
        out3[i] = inp1[ip + 1];
        ip += 2;
        out4[i] = inp1[ip];
        out5[i] = inp1[ip + 1];
        ip += 2;
        out6[i] = inp1[ip];
        out7[i] = inp1[ip + 1];
        ip += 2;
    }
    ip = 0usize;
    for i in CHUNK_SIZE/4..(3*CHUNK_SIZE/8) {
        out0[i] = inp2[ip];
        out1[i] = inp2[ip + 1];
        ip += 2;
        out2[i] = inp2[ip];
        out3[i] = inp2[ip + 1];
        ip += 2;
        out4[i] = inp2[ip];
        out5[i] = inp2[ip + 1];
        ip += 2;
        out6[i] = inp2[ip];
        out7[i] = inp2[ip + 1];
        ip += 2;
    }
    ip = 0usize;
    for i in (3*CHUNK_SIZE/8)..CHUNK_SIZE/2 {
        out0[i] = inp3[ip];
        out1[i] = inp3[ip + 1];
        ip += 2;
        out2[i] = inp3[ip];
        out3[i] = inp3[ip + 1];
        ip += 2;
        out4[i] = inp3[ip];
        out5[i] = inp3[ip + 1];
        ip += 2;
        out6[i] = inp3[ip];
        out7[i] = inp3[ip + 1];
        ip += 2;
    }
    ip = 0usize;
    for i in CHUNK_SIZE/2..(5*CHUNK_SIZE/8) {
        out0[i] = inp4[ip];
        out1[i] = inp4[ip + 1];
        ip += 2;
        out2[i] = inp4[ip];
        out3[i] = inp4[ip + 1];
        ip += 2;
        out4[i] = inp4[ip];
        out5[i] = inp4[ip + 1];
        ip += 2;
        out6[i] = inp4[ip];
        out7[i] = inp4[ip + 1];
        ip += 2;
    }
    ip = 0usize;
    for i in (5*CHUNK_SIZE/8)..(3*CHUNK_SIZE)/4 {
        out0[i] = inp5[ip];
        out1[i] = inp5[ip + 1];
        ip += 2;
        out2[i] = inp5[ip];
        out3[i] = inp5[ip + 1];
        ip += 2;
        out4[i] = inp5[ip];
        out5[i] = inp5[ip + 1];
        ip += 2;
        out6[i] = inp5[ip];
        out7[i] = inp5[ip + 1];
        ip += 2;
    }
    ip = 0usize;
    for i in (3*CHUNK_SIZE)/4..(7*CHUNK_SIZE)/8 {
        out0[i] = inp6[ip];
        out1[i] = inp6[ip + 1];
        ip += 2;
        out2[i] = inp6[ip];
        out3[i] = inp6[ip + 1];
        ip += 2;
        out4[i] = inp6[ip];
        out5[i] = inp6[ip + 1];
        ip += 2;
        out6[i] = inp6[ip];
        out7[i] = inp6[ip + 1];
        ip += 2;
    }
    ip = 0usize;
    for i in (7*CHUNK_SIZE)/8..CHUNK_SIZE {
        out0[i] = inp7[ip];
        out1[i] = inp7[ip + 1];
        ip += 2;
        out2[i] = inp7[ip];
        out3[i] = inp7[ip + 1];
        ip += 2;
        out4[i] = inp7[ip];
        out5[i] = inp7[ip + 1];
        ip += 2;
        out6[i] = inp7[ip];
        out7[i] = inp7[ip + 1];
        ip += 2;
    }
}
fn twiddle2(a: usize, b: usize, inp: &'static FftBuf, out: &'static FftBuf, twiddles: &'static [Complex64], m:usize) {
    debug_assert_eq!(a % CHUNK_SIZE, 0);
    debug_assert_eq!(b % CHUNK_SIZE, 0);
    debug_assert_eq!(b - a, CHUNK_SIZE);

    let chunk0 = a / CHUNK_SIZE;
    let chunk1 = (a + m) / CHUNK_SIZE;

    let inp0 = inp[chunk0].lock().unwrap();
    let inp1 = inp[chunk1].lock().unwrap();
    let mut out0 = out[chunk0].lock().unwrap();
    let mut out1 = out[chunk1].lock().unwrap();

    for j in 0..CHUNK_SIZE {
        let i = a + j;
        let l1 = i;

        let x0 = inp0[j];
        let x1 = inp1[j];

        let x1 = twiddles[l1] * x1;

        out0[j] = x0 + x1;
        out1[j] = x0 - x1;
    }
}
fn twiddle4(a: usize, b: usize, inp: &'static FftBuf, out: &'static FftBuf, twiddles: &'static [Complex64], m:usize) {
    debug_assert_eq!(a % CHUNK_SIZE, 0);
    debug_assert_eq!(b % CHUNK_SIZE, 0);
    debug_assert_eq!(b - a, CHUNK_SIZE);

    let c0 = a / CHUNK_SIZE;
    let c1 = (a + m) / CHUNK_SIZE;
    let c2 = (a + 2 * m) / CHUNK_SIZE;
    let c3 = (a + 3 * m) / CHUNK_SIZE;

    let inp0 = inp[c0].lock().unwrap();
    let inp1 = inp[c1].lock().unwrap();
    let inp2 = inp[c2].lock().unwrap();
    let inp3 = inp[c3].lock().unwrap();

    let mut out0 = out[c0].lock().unwrap();
    let mut out1 = out[c1].lock().unwrap();
    let mut out2 = out[c2].lock().unwrap();
    let mut out3 = out[c3].lock().unwrap();

    for j in 0..CHUNK_SIZE {
        let i = a + j;
        let l1 = i;

        // Even inputs: 0 and 2
        let x0 = inp0[j];
        let x2 = twiddles[2 * l1] * inp2[j];

        let r0 = x0 + x2;
        let r2 = x0 - x2;

        // Odd inputs: 1 and 3
        let x1 = twiddles[l1] * inp1[j];
        let x3 = twiddles[3 * l1] * inp3[j];

        let r1 = x1 + x3;
        let r3 = x1 - x3;

        out0[j] = r0 + r1;
        out2[j] = r0 - r1;

        let i_r3 = Complex64::new(-r3.im, r3.re);

        out1[j] = r2 + i_r3;
        out3[j] = r2 - i_r3;
    }
}
fn twiddle8(a: usize, b: usize, inp: &'static FftBuf, out: &'static FftBuf, twiddles: &'static [Complex64], m:usize) {
    debug_assert_eq!(a % CHUNK_SIZE, 0);
    debug_assert_eq!(b % CHUNK_SIZE, 0);
    debug_assert_eq!(b - a, CHUNK_SIZE);

    let c0 = a / CHUNK_SIZE;
    let c1 = (a + m) / CHUNK_SIZE;
    let c2 = (a + 2 * m) / CHUNK_SIZE;
    let c3 = (a + 3 * m) / CHUNK_SIZE;
    let c4 = (a + 4 * m) / CHUNK_SIZE;
    let c5 = (a + 5 * m) / CHUNK_SIZE;
    let c6 = (a + 6 * m) / CHUNK_SIZE;
    let c7 = (a + 7 * m) / CHUNK_SIZE;

    let inp0 = inp[c0].lock().unwrap();
    let inp1 = inp[c1].lock().unwrap();
    let inp2 = inp[c2].lock().unwrap();
    let inp3 = inp[c3].lock().unwrap();
    let inp4 = inp[c4].lock().unwrap();
    let inp5 = inp[c5].lock().unwrap();
    let inp6 = inp[c6].lock().unwrap();
    let inp7 = inp[c7].lock().unwrap();

    let mut out0 = out[c0].lock().unwrap();
    let mut out1 = out[c1].lock().unwrap();
    let mut out2 = out[c2].lock().unwrap();
    let mut out3 = out[c3].lock().unwrap();
    let mut out4 = out[c4].lock().unwrap();
    let mut out5 = out[c5].lock().unwrap();
    let mut out6 = out[c6].lock().unwrap();
    let mut out7 = out[c7].lock().unwrap();

    for j in 0..CHUNK_SIZE {
        let i = a + j;
        let l1 = i;

        // ------------------------------------------------------------
        // Even inputs: 0, 2, 4, 6
        // ------------------------------------------------------------
        let x0 = inp0[j];
        let x4 = twiddles[4 * l1] * inp4[j];

        let r2_0 = x0 + x4;
        let r2_4 = x0 - x4;

        let x2 = twiddles[2 * l1] * inp2[j];
        let x6 = twiddles[6 * l1] * inp6[j];

        let r2_2 = x2 + x6;
        let r2_6 = x2 - x6;

        let r1_0 = r2_0 + r2_2;
        let r1_4 = r2_0 - r2_2;

        // r2_4 - i*r2_6
        let minus_i_r2_6 = Complex64::new(r2_6.im, -r2_6.re);

        let r1_2 = r2_4 + minus_i_r2_6;
        let r1_6 = r2_4 - minus_i_r2_6;

        // ------------------------------------------------------------
        // Odd inputs: 1, 3, 5, 7
        // ------------------------------------------------------------

        let x1 = twiddles[l1] * inp1[j];
        let x5 = twiddles[5 * l1] * inp5[j];

        let r2_1 = x1 + x5;
        let r2_5 = x1 - x5;

        let x3 = twiddles[3 * l1] * inp3[j];
        let x7 = twiddles[7 * l1] * inp7[j];

        let r2_3 = x3 + x7;
        let r2_7 = x3 - x7;

        let r1_1 = r2_1 + r2_3;
        let r1_5 = r2_1 - r2_3;

        // r2_5 - i*r2_7
        let minus_i_r2_7 = Complex64::new(r2_7.im, -r2_7.re);

        let r1_3 = r2_5 + minus_i_r2_7;
        let r1_7 = r2_5 - minus_i_r2_7;

        // ------------------------------------------------------------
        // Final radix-8 butterfly
        // ------------------------------------------------------------

        out0[j] = r1_0 + r1_1;
        out4[j] = r1_0 - r1_1;

        let t3 = Complex64::new(
            FRAC_1_SQRT_2 * (r1_3.re + r1_3.im),
            FRAC_1_SQRT_2 * (r1_3.im - r1_3.re),
        );

        out1[j] = r1_2 + t3;
        out5[j] = r1_2 - t3;

        let minus_i_r1_5 =
            Complex64::new(r1_5.im, -r1_5.re);

        out2[j] = r1_4 + minus_i_r1_5;
        out6[j] = r1_4 - minus_i_r1_5;

        let t7 = Complex64::new(
            FRAC_1_SQRT_2 * (r1_7.im - r1_7.re),
            FRAC_1_SQRT_2 * (r1_7.re + r1_7.im),
        );

        out3[j] = Complex64::new(
            r1_6.re + t7.re,
            r1_6.im - t7.im,
        );

        out7[j] = Complex64::new(
            r1_6.re - t7.re,
            r1_6.im + t7.im,
        );
    }
}
fn fft_2(inp: &[Complex64], out: &mut [Complex64]) {
    out[0].re = inp[0].re + inp[1].re;
    out[0].im = inp[0].im + inp[1].im;
    out[1].re = inp[0].re - inp[1].re;
    out[1].im = inp[0].im - inp[1].im;
}
fn fft_4(inp: &[Complex64], out: &mut [Complex64]) {
    let r2_0 = inp[0].re;
    let i2_0 = inp[0].im;
    let r2_1 = inp[1].re;
    let i2_1 = inp[1].im;
    let r2_2 = inp[2].re;
    let i2_2 = inp[2].im;
    let r2_3 = inp[3].re;
    let i2_3 = inp[3].im;
    let r1_0 = r2_0 + r2_2;
    let i1_0 = i2_0 + i2_2;
    let r1_2 = r2_0 - r2_2;
    let i1_2 = i2_0 - i2_2;
    let r1_1 = r2_1 + r2_3;
    let i1_1 = i2_1 + i2_3;
    let r1_3 = r2_1 - r2_3;
    let i1_3 = i2_1 - i2_3;
    out[0].re = r1_0 + r1_1;
    out[0].im = i1_0 + i1_1;
    out[2].re = r1_0 - r1_1;
    out[2].im = i1_0 - i1_1;
    out[1].re = r1_2 + i1_3;
    out[1].im = i1_2 - r1_3;
    out[3].re = r1_2 - i1_3;
    out[3].im = i1_2 + r1_3;
}
fn fft_8(input: &[Complex64], out: &mut [Complex64]) {
    let r3_0 = input[0].re;
    let i3_0 = input[0].im;
    let r3_4 = input[4].re;
    let i3_4 = input[4].im;
    let r2_0 = r3_0 + r3_4;
    let i2_0 = i3_0 + i3_4;
    let r2_4 = r3_0 - r3_4;
    let i2_4 = i3_0 - i3_4;
    let r3_2 = input[2].re;
    let i3_2 = input[2].im;
    let r3_6 = input[6].re;
    let i3_6 = input[6].im;
    let r2_2 = r3_2 + r3_6;
    let i2_2 = i3_2 + i3_6;
    let r2_6 = r3_2 - r3_6;
    let i2_6 = i3_2 - i3_6;
    let r1_0 = r2_0 + r2_2;
    let i1_0 = i2_0 + i2_2;
    let r1_4 = r2_0 - r2_2;
    let i1_4 = i2_0 - i2_2;
    let r1_2 = r2_4 + i2_6;
    let i1_2 = i2_4 - r2_6;
    let r1_6 = r2_4 - i2_6;
    let i1_6 = i2_4 + r2_6;
    let r3_1 = input[1].re;
    let i3_1 = input[1].im;
    let r3_5 = input[5].re;
    let i3_5 = input[5].im;
    let r2_1 = r3_1 + r3_5;
    let i2_1 = i3_1 + i3_5;
    let r2_5 = r3_1 - r3_5;
    let i2_5 = i3_1 - i3_5;
    let r3_3 = input[3].re;
    let i3_3 = input[3].im;
    let r3_7 = input[7].re;
    let i3_7 = input[7].im;
    let r2_3 = r3_3 + r3_7;
    let i2_3 = i3_3 + i3_7;
    let r2_7 = r3_3 - r3_7;
    let i2_7 = i3_3 - i3_7;
    let r1_1 = r2_1 + r2_3;
    let i1_1 = i2_1 + i2_3;
    let r1_5 = r2_1 - r2_3;
    let i1_5 = i2_1 - i2_3;
    let r1_3 = r2_5 + i2_7;
    let i1_3 = i2_5 - r2_7;
    let r1_7 = r2_5 - i2_7;
    let i1_7 = i2_5 + r2_7;
    out[0].re = r1_0 + r1_1;
    out[0].im = i1_0 + i1_1;
    out[4].re = r1_0 - r1_1;
    out[4].im = i1_0 - i1_1;
    let tmpr = FRAC_1_SQRT_2 * (r1_3 + i1_3);
    let tmpi = FRAC_1_SQRT_2 * (i1_3 - r1_3);
    out[1].re = r1_2 + tmpr;
    out[1].im = i1_2 + tmpi;
    out[5].re = r1_2 - tmpr;
    out[5].im = i1_2 - tmpi;
    out[2].re = r1_4 + i1_5;
    out[2].im = i1_4 - r1_5;
    out[6].re = r1_4 - i1_5;
    out[6].im = i1_4 + r1_5;
    let tmpr = FRAC_1_SQRT_2 * (i1_7 - r1_7);
    let tmpi = FRAC_1_SQRT_2 * (r1_7 + i1_7);
    out[3].re = r1_6 + tmpr;
    out[3].im = i1_6 - tmpi;
    out[7].re = r1_6 - tmpr;
    out[7].im = i1_6 + tmpi;
}

fn unshuffle2_local(inp: &[Complex64], out: &mut [Complex64], m: usize) {todo!()}
fn unshuffle4_local(inp: &[Complex64], out: &mut [Complex64], m: usize) {todo!()}
fn unshuffle8_local(inp: &[Complex64], out: &mut [Complex64], m: usize) {todo!()}

fn twiddle2_local(inp: &[Complex64], out: &mut [Complex64], twiddles: &'static [Complex64], m:usize) {todo!()}
fn twiddle4_local(inp: &[Complex64], out: &mut [Complex64], twiddles: &'static [Complex64], m:usize) {todo!()}
fn twiddle8_local(inp: &[Complex64], out: &mut [Complex64], twiddles: &'static [Complex64], m:usize) {todo!()}


fn unshuffle(radix: usize, a: usize, b: usize, inp: &'static FftBuf, out: &'static FftBuf, m: usize) {
    debug_assert_eq!(a % CHUNK_SIZE, 0);
    debug_assert_eq!(b % CHUNK_SIZE, 0);
    if b - a <= CHUNK_SIZE {
        match radix {
            8 => return unshuffle8(a, inp, out, m),
            4 => return unshuffle4(a, inp, out, m),
            2 => return unshuffle2(a, inp, out, m),
            _ => unreachable!(),
        }
    }
    let mid = (a + b) / 2;
    unshuffle(radix, a, mid, inp, out, m);
    unshuffle(radix, mid, b, inp, out, m);
}

fn twiddle(radix: usize, a: usize, b: usize, inp: &'static FftBuf, out: &'static FftBuf, twiddles: &'static [Complex64], m: usize) {
    debug_assert_eq!(a % CHUNK_SIZE, 0);
    debug_assert_eq!(b % CHUNK_SIZE, 0);
    if b - a <= CHUNK_SIZE {
        match radix {
            8 => return twiddle8(a, b, inp, out, twiddles, m),
            4 => return twiddle4(a, b, inp, out, twiddles, m),
            2 => return twiddle2(a, b, inp, out, twiddles, m),
            _ => unreachable!(),
        }
    }
    let mid = (a + b) / 2;
    twiddle(radix, a, mid, inp, out, twiddles, m);
    twiddle(radix, mid, b, inp, out, twiddles, m);
}

// top-level will be size 
fn fft_local(radices: &[usize], inp: &mut [Complex64], out: &mut[Complex64], twiddles: &'static [Complex64]) {
    let n = inp.len();
    if n == 8 {
        return fft_8(inp, out);
    } else if n == 4 {
        return fft_4(inp, out);
    } else  if n == 2 {
        return fft_2(inp, out);
    }

    let radix = radices[0];
    let m = n/radix;
    match radix {
        8 => unshuffle8_local(inp, out, m),
        4 => unshuffle4_local(inp, out, m),
        2 => unshuffle2_local(inp, out, m),
        _ => unreachable!(),
    }
    for k in (0..n).step_by(m) {
        fft_local(&radices[1..], &mut out[k..k+m], &mut inp[k..k+m], twiddles);
    }
    match radix {
        8 => return twiddle8_local(inp, out, twiddles, m),
        4 => return twiddle4_local(inp, out, twiddles, m),
        2 => return twiddle2_local(inp, out, twiddles, m),
        _ => unreachable!(),
    }
}
/*
    assert!(!radices.is_empty()); (after base-case)
    assert_eq!( radices.iter().product::<usize>() * CHUNK_SIZE, n );
    debug_assert_eq!(k % CHUNK_SIZE, 0);
    debug_assert_eq!(m % CHUNK_SIZE, 0);
    assert!(!radices.is_empty());
    assert!(matches!(radices[0], 2 | 4 | 8));
    assert_eq!(n % radices[0], 0);
    assert_eq!(
    input_chunks,
    expected_chunks,
);
*/
pub(crate) fn fft(radices: &'static [usize], inp: &'static FftBuf, out: &'static FftBuf, twiddles: &'static [Complex64]) {
    // single chunk base-case
    if inp.len() == 1 {
        let mut inp = inp[0].lock().unwrap();
        let mut out = out[0].lock().unwrap();
        fft_local(radices, &mut *inp, &mut *out, twiddles);
        return; 
    }

    let n = inp.len() * CHUNK_SIZE;
    let radix = radices[0];
    let m = n/radix;

    unshuffle(radix, 0, m, inp, out, m);

    for k in (0..n).step_by(m) {
        debug_assert_eq!(k % CHUNK_SIZE, 0);
        debug_assert_eq!(m % CHUNK_SIZE, 0);

        let start = k / CHUNK_SIZE;
        let end = start + m / CHUNK_SIZE;
        fft(&radices[1..], &out[start..end], &inp[start..end], twiddles);
    }

    twiddle(radix, 0, m, inp, out, twiddles, m); // have to add to sync-sensitivity!
}
