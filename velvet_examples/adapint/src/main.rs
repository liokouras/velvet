use velvet::prelude::*;
use std::time::Instant;
use std::env;

include!(concat!(env!("OUT_DIR"), "/velvet_app.rs"));

const THRESHOLD: u64 = 500;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 5 {
        println!("usage for adapint: cargo run [cargo_options] [velvet|seq] [a] [b] [epsilon]");
        println!("example: cargo run --release velvet 0.0 640000 0.0001");
        return;
    }

    let app = &args[1];
    let a: f64 = args[2].parse().unwrap();
    let b: f64 = args[3].parse().unwrap();
    let epsilon: f64 = args[4].parse().unwrap();

    if app.eq("velvet") {
        velvet_main(a, b, epsilon);
    } else if app.eq("seq") {
        let start_seq = Instant::now();
        let oracle = adapint_seq(a, b, epsilon);
        let end_seq = start_seq.elapsed();
        println!("ADAPINT({},{},{}) = {} IN SEQUENTIAL TIME: {}", a, b, epsilon, oracle, end_seq.as_secs_f32());
    } else {
        eprint!("Unrecognized app: {}", app);
    }
}

#[inline(always)]
fn f(x: f64) -> f64 {
    x.sin() * 0.1 * x
}

#[velvet_main(adapint)]
fn velvet_main(a: f64, b: f64, epsilon: f64) {
    let start = Instant::now();
    let res = adapint(a, b, epsilon);
    let end = start.elapsed();

    println!("ADAPINT({},{},{}) = {} IN PARALLEL TIME: {}", a, b, epsilon, res, end.as_secs_f32());
    println!("Velvet queue backend: {} ; Sequential threshold = {}", velvet_get_queue_name().as_str(), THRESHOLD);
}


#[spawnable]
fn adapint(a: f64, b: f64, epsilon: f64) -> f64 {
    let delta = (b - a) / 2.0;
    let deltahalf = delta / 2.0;
    let mid = delta + a;
    let fa = f(a);
    let fb = f(b);
    let fmid = f(mid);
    let total = delta * (fa + fb);
    let left = deltahalf * (fa + fmid);
    let right = deltahalf * (fb + fmid);
    let mut diff = total - (left + right);
    if diff < 0.0 {
        diff = -diff;
    }
    
    if diff < epsilon {
        return total
    }

    if diff <= THRESHOLD as f64 {
        let i1 = adapint_seq(mid, b, epsilon);
        let i2 = adapint_seq(a, mid, epsilon);
        return i1 + i2;
    }

    let i1 = adapint(mid, b, epsilon);
    let i2 = adapint(a, mid, epsilon);
    return i1 + i2;
}

fn adapint_seq(a: f64, b: f64, epsilon: f64) -> f64 {
    let delta = (b - a) / 2.0;
    let deltahalf = delta / 2.0;
    let mid = delta + a;
    let fa = f(a);
    let fb = f(b);
    let fmid = f(mid);
    let total = delta * (fa + fb);
    let left = deltahalf * (fa + fmid);
    let right = deltahalf * (fb + fmid);
    let mut diff = total - (left + right);
    if diff < 0.0 {
        diff = -diff;
    }
    
    if diff < epsilon {
        return total
    } else {
        let i2 = adapint_seq(a, mid, epsilon);
        let i1 = adapint_seq(mid, b, epsilon);
        return i1 + i2; 
    }
}