use velvet::prelude::*;
use std::time::Instant;
use std::env;

include!(concat!(env!("OUT_DIR"), "/velvet_app.rs"));

const THRESHOLD: usize = 6;

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 {
        println!("usage for nqueens: cargo run [cargo_options] [velvet|seq] [n]");
        println!("example: cargo run --release velvet 15");
        return;
    }

    let app = &args[1];
    let n: usize = args[2].parse().unwrap();
    
    if app.eq("velvet") {
        velvet_main(n);
    } else if app.eq("seq") {
        let mut board = vec![0;n];

        let start_seq = Instant::now();
        let oracle = nqueens(&mut board, 0, n);
        let end_seq = start_seq.elapsed();
        println!("NQUEENS({}) = {} IN SEQUENTIAL TIME: {}", n, oracle, end_seq.as_secs_f32());
    } else {
        eprintln!("Unrecognised app {}", app);
    }
}

#[velvet_main(nqueens_spawn)]
fn velvet_main(n: usize) {
    let board = vec![0;n];

    let start = Instant::now();
    let res = nqueens_spawn(board, 0, n);
    let end = start.elapsed();

    println!("NQUEENS({}) = {} IN PARALLEL TIME: {}", n, res, end.as_secs_f32());
    println!("Velvet queue backend: {} ; Sequential threshold = {}", velvet_get_queue_name().as_str(), THRESHOLD);
}

#[spawnable]
fn nqueens_spawn(mut board: Vec<u8>, row: usize, size: usize) -> usize {
    if row > THRESHOLD {
        return nqueens(&mut board, row, size);
    }
    if row >= size {
        return 1;
    }

    let mut solutions = 0;

    'try_new_row: for q in 0..size {
        // incremental conflict check
        for i in 0..row {
            let p = board[i] as isize - q as isize;
            let d = (row - i) as isize;
            if p == 0 || p == d || p == -d {
                continue 'try_new_row;
            }
        }

        // par recursion: copy board
        let mut new_board = board.clone();
        new_board[row] = q as u8;
        solutions += nqueens_spawn(new_board, row + 1, size);
    }

    solutions
}

fn nqueens(board: &mut [u8], row: usize, size: usize) -> usize {
    if row >= size {
        return 1;
    }

    let mut solutions = 0;

    'try_new_row: for q in 0..size {
        // incremental conflict check
        for i in 0..row {
            let p = board[i] as isize - q as isize;
            let d = (row - i) as isize;
            if p == 0 || p == d || p == -d {
                continue 'try_new_row;
            }
        }

        // sequential recursion: reuse board
        board[row] = q as u8;
        solutions += nqueens(board, row + 1, size);
    }

    solutions
}