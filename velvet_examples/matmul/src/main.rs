mod matrix_par;
use matrix_par::Matrix;
mod matrix_seq;

use std::{sync::Arc, env, time::Instant};

use velvet::prelude::*;
include!(concat!(env!("OUT_DIR"), "/velvet_app.rs"));


// pub(crate) type Real = f32;
pub(crate) type Real = f64;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 4 {
        println!("usage for matmul: cargo run [cargo_options] [velvet|seq] [depth] [dim]");
        println!("example: cargo run --release velvet 2 4");
        return;
    }
    let app = &args[1];
    let depth: usize = args[2].parse().unwrap();
    let dim: usize = args[3].parse().unwrap();

    if app.eq("seq") {
        seq_main(depth, dim);
    } else if app.eq("velvet") {
        velvet_main(depth, dim);
    } else {
        println!("Unrecognised app: {}", app);
    }
}

fn seq_main(depth: usize, dim: usize) {
    let matrix_a = matrix_seq::Matrix::new(depth, dim, 1.0);
    let matrix_b = matrix_seq::Matrix::new(depth, dim, 2.0);
    let mut matrix_c = matrix_seq::Matrix::new(depth, dim, 0.0);

    let start = Instant::now();
    matrix_c.matmul(depth, &matrix_a, &matrix_b);
    let end = start.elapsed();

    let full_dim: usize = 2_usize.pow(depth.try_into().unwrap()) * dim;
    let ok = matrix_c._check((full_dim * 2) as Real);

    println!("MATMUL({}x{}); OK = {} IN SEQUENTIAL TIME: {}", full_dim, full_dim, ok, end.as_secs_f32());
}

#[velvet_main(spawn_matmul)]
fn velvet_main(depth: usize, dim: usize) {
    let matrix_a = Arc::new(Matrix::new(depth, dim, 1.0, false));
    let matrix_b= Arc::new(Matrix::new(depth, dim, 2.0, false));
    let matrix_c = Arc::new(Matrix::new(depth, dim, 0.0, true));

    let start = Instant::now();
    matrix_c.spawn_matmul(depth, &matrix_a, &matrix_b);
    let end = start.elapsed();

    let full_dim: usize = 2_usize.pow(depth.try_into().unwrap()) * dim;
    let ok = matrix_c._check((full_dim * 2) as Real);

    println!("MATMUL({}x{}); OK = {} IN PARALLEL TIME: {}", full_dim, full_dim, ok, end.as_secs_f32());
    println!("Velvet queue backend: {} ; Sequential threshold = {}x{}", velvet_get_queue_name().as_str(), dim, dim);    
}