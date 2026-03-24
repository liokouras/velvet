use velvet::prelude::*;
use std::time::Instant;
use std::env;

include!(concat!(env!("OUT_DIR"), "/velvet_app.rs"));

const THRESHOLD: u64 = 25;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        println!("usage for fib: cargo run [cargo_options] [velvet|seq] [n]");
        println!("example: cargo run --release velvet 42");
        return;
    }

    let app = &args[1];
    let n: u64 = args[2].parse().unwrap();

    if app.eq("velvet") {
        velvet_main(n);
    } else if app.eq("seq") {
        let start_seq = Instant::now();
        let oracle = fib_seq(n);
        let end_seq = start_seq.elapsed();
        println!("FIB({}) = {} IN SEQUENTIAL TIME: {}", n, oracle, end_seq.as_secs_f32());
    } else {
        eprint!("Unrecognized app: {}", app);
    }
}

#[velvet_main(fib)]
fn velvet_main(n: u64) {
    let start = Instant::now();
    let res = fib(n);
    let end = start.elapsed();
    println!("FIB({}) = {} IN PARALLEL TIME: {}", n, res, end.as_secs_f32());
    println!("Velvet queue backend: {} ; Sequential threshold = {}", velvet_get_queue_name().as_str(), THRESHOLD);
}

#[spawnable]
fn fib(n: u64) -> u64 {
    if n < THRESHOLD { return fib_seq(n); }
    let r2 = fib(n-1);
    let r1 = fib(n-2);
    return r1 + r2;
}

fn fib_seq(n: u64) -> u64 {
    if n < 2 { return n; }
    let r1 = fib_seq(n-2);
    let r2 = fib_seq(n-1);
    return r1 + r2;
}