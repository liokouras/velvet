use std::{env, time::Instant, sync::{atomic::{AtomicUsize, Ordering}, OnceLock}};
use rand::{SeedableRng, rngs::StdRng, distr::{Distribution, Uniform}};

use velvet::prelude::*;

include!(concat!(env!("OUT_DIR"), "/velvet_app.rs"));

static DISTANCE:OnceLock<DistanceTable> = OnceLock::new();
static MINIMUM:AtomicUsize = AtomicUsize::new(usize::MAX);

const THRESHOLD: usize = 6;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 4 {
        println!("usage for tsp: cargo run [cargo_options] [velvet|seq] [number_of_towns] [random_seed]");
        println!("example: cargo run --release velvet 20 42");
        return;
    }

    let app = &args[1];
    let ntowns: usize = args[2].parse().unwrap();
    let seed: u64 = args[3].parse().unwrap();

    let distance = DistanceTable::generate(ntowns, seed);
    match DISTANCE.set(distance) {
        Err(_) => {
            println!("could not initialise global distance table variable. exiting...");
            return;
        },
        Ok(()) => (),
    }

    if app.eq("velvet") {
        velvet_main(ntowns);
    } else if app.eq("seq") {
        let path: u128 = 1u128; 
        let length = 0;

        let start = Instant::now();
        tsp_seq(1, 0, path, length);
        let elapsed = start.elapsed();

        println!("TSP({}) = {} IN SEQUENTIAL TIME: {}", ntowns, MINIMUM.load(Ordering::Relaxed), elapsed.as_secs_f32());
    } else {
        eprint!("Unrecognized app: {}", app);
    }
}

#[velvet_main(tsp_spawn)]
fn velvet_main(ntowns: usize) {
    let path: u128 = 1u128; 
    let length = 0;

    let start = Instant::now();
    tsp_spawn(1, 0, path, length);
    let elapsed = start.elapsed();

    println!("TSP({}) = {} IN PARALLEL TIME: {}", ntowns, MINIMUM.load(Ordering::Relaxed), elapsed.as_secs_f32());
    println!("Velvet queue backend: {} ; Sequential threshold = {}", velvet_get_queue_name().as_str(), THRESHOLD);
}

fn tsp_seq(hops: usize, last: usize, path: u128, length: usize) {
    let distance = DISTANCE.get().unwrap();
    let ntowns = distance.ntowns;

    if length + distance.lower_bounds[ntowns - hops] >= MINIMUM.load(Ordering::Relaxed) {
        // stop searching, this path is too long...
        return;
    } else if hops == ntowns {
        // found a full route better than current best route,
        MINIMUM.store(length, Ordering::Relaxed);
        return;
    }
    
    // try all cities not on the path, in "nearest-city-first" order
    for i in 0..ntowns {
        let city = distance.to_city[last * ntowns +  i];
        let city_bit = 1u128 << city;

        if city != last && (path & city_bit) == 0 {
            let dist = distance.dist[last * ntowns + i];
            let new_path = path | city_bit; 
            tsp_seq(hops + 1, city, new_path, length + dist);
        }
    }
}

#[spawnable]
fn tsp_spawn(hops: usize, last: usize, path: u128, length: usize) {
    let distance = DISTANCE.get().unwrap();
    let ntowns = distance.ntowns;

    if length + distance.lower_bounds[ntowns - hops] >= MINIMUM.load(Ordering::Relaxed) {
        // stop searching, this path is too long...
        return;
    } else if hops == ntowns {
        // found a full route better than current best route,
        MINIMUM.store(length, Ordering::Relaxed);
        return;
    }

    if hops > THRESHOLD {
        return tsp_seq(hops, last, path, length);
    }
    
    // try all cities not on the path, in "nearest-city-first" order
    for i in (0..ntowns).rev() {
        let city = distance.to_city[last * ntowns +  i];
        let city_bit = 1u128 << city;

        if city != last && (path & city_bit) == 0 {
            let dist = distance.dist[last * ntowns + i];
            let new_path = path | city_bit; 
            tsp_spawn(hops + 1, city, new_path, length + dist);
        }
    }
}

struct Coord {
    x: usize,
    y: usize,
}

struct DistanceTable {
    ntowns: usize,
    lower_bounds: Vec<usize>,
    to_city: Vec<usize>,
    dist: Vec<usize>,
}

impl DistanceTable {
    fn generate(ntowns: usize, seed:u64) -> DistanceTable {
        let mut to_city = vec![0;ntowns * ntowns];
        let mut dist = vec![0;ntowns * ntowns];
        let mut lower_bounds =  vec![0;ntowns];

        let mut temp_dist = vec![0;ntowns];
        let mut towns = Vec::with_capacity(ntowns);
        let mut min_dists = vec![0;ntowns * ntowns];

        let mut dx;
        let mut dy;
        let mut x = 0;
        let mut min_dist_count = 0;
        let mut tmp;

        let range: Uniform<usize> = Uniform::try_from(0..100).unwrap();
        let mut rng = StdRng::seed_from_u64(seed); 
        for _ in 0..ntowns {
            let x = range.sample(&mut rng);
            let y = range.sample(&mut rng);
            towns.push(Coord{x, y}); 
        }

        for i in 0..ntowns {
            for j in 0..ntowns {
                dx = towns[i].x as i32 - towns[j].x as i32;
                dy = towns[i].y as i32 - towns[j].y as i32;
                let dist = (dx * dx + dy * dy).isqrt();
                temp_dist[j] = dist as usize;
                if i != j && temp_dist[j] != 0 {
                    min_dists[min_dist_count] = temp_dist[j];
                    min_dist_count += 1;
                }
            }

            // Sort pairs[i]: nearest city first.
            for j in 0..ntowns {
                tmp = usize::MAX;
                for k in 0..ntowns {
                    if temp_dist[k] < tmp {
                        tmp = temp_dist[k];
                        x = k;
                    }
                }
                temp_dist[x] = usize::MAX;
                to_city[i*ntowns + j] = x;
                dist[i*ntowns + j] = tmp;
            }
        }

        DistanceTable::sort(&mut min_dists);

        for i in 0..ntowns {
            lower_bounds[i] = DistanceTable::calc_lower_bound(i, &min_dists);
        }


        DistanceTable{ ntowns, lower_bounds, to_city, dist }
    }

    fn sort(vec: &mut Vec<usize>) {
        for i in 0..vec.len() {
            DistanceTable::put_min(vec, i);
        }
    }

    fn put_min(vec: &mut Vec<usize>, pos: usize) {
        let mut minpos = pos;
        let mut min = usize::MAX;
        for i in pos..vec.len() {
            if vec[i] == 0 {
                vec[i] = usize::MAX;
            }
            if vec[i] < min {
                minpos = i;
                min = vec[i];
            }
        }
        let tmp = vec[pos];
        vec[pos] = vec[minpos];
        vec[minpos] = tmp;
    }

    fn calc_lower_bound(hops: usize, table: &Vec<usize>) -> usize {
        let mut res = 0;
        for i in 0..hops {
            res += table[i] as usize;
        }
        res
    }
}