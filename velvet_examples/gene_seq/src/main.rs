use bio::alignment::pairwise::Aligner;
use bio::io::fasta;
use std::{collections::HashMap, error::Error};

use velvet::prelude::*;

include!(concat!(env!("OUT_DIR"), "/velvet_app.rs"));


const TOP_K: usize = 20; // Satin's input_02.txt has scores = 20. TODO parameterise
const THRESHOLD: usize = 25; // Satin's input_02.txt has threshold = 4. TODO parameterise

#[derive(Debug)]
struct Sequence {
    id: String,
    seq: Vec<u8>,
}

type Hit = (String, i32); // DB-ID and score
type QueryResult = (String, Vec<Hit>); // Query-ID and Hits

fn load_fasta(path: &str) -> Result<Vec<Sequence>, Box<dyn Error>> {
    let reader = fasta::Reader::from_file(path)?;

    let mut sequences = Vec::new();

    for record in reader.records() {
        let record = record?;

        sequences.push(Sequence {
            id: record.id().to_owned(),
            seq: record.seq().to_vec(),
        });
    }

    Ok(sequences)
}

// TODO: write to file intead of to stdout?
fn dump(results: &[QueryResult]) {
    for result in results {
        println!();
        println!("> {}", result.0);
        println!();

        for (i, hit) in result.1.iter().enumerate() {
            println!(
                "[{}] : {} : >{}",
                i + 1,
                hit.1,
                hit.0
            );
        }
    }
}

fn process_leaf(queries: &'static [Sequence], database: &'static [Sequence]) -> Vec<QueryResult> {
    // build aligner
    let max_query_len = queries.iter().map(|s| s.seq.len()).max().unwrap_or(0);
    let max_db_len = database.iter().map(|s| s.seq.len()).max().unwrap_or(0);
    // Satin input_02.txt (TODO parameterise!): match =  5; mismatch = -4; gap = -4
    let score = |a: u8, b: u8| if a == b { 5 } else { -4 };
    // rust-bio uses separate gap-open and gap-extension penalties. Setting both to -4.
    let mut aligner = Aligner::with_capacity(
        max_query_len,
        max_db_len,
        -4, // gap open
        -4, // gap_extend
        score,
    );

    let mut results = Vec::with_capacity(queries.len());

    for query in queries {
        let mut hits = Vec::new();

        for database_sequence in database {
            // use smith-waterman 'local'
            let score = aligner.local(&query.seq, &database_sequence.seq).score;

            // only keep positive-scoring alignments
            if score > 0 {
                hits.push((database_sequence.id.clone(), score));
            }
        }

        // highest scores first
        hits.sort_by(|a, b| {
            b.1
                .cmp(&a.1)
                .then_with(|| a.0.cmp(&b.0))
        });

        hits.truncate(TOP_K);

        results.push((query.id.clone(),hits));
    }

    results
}

fn combine(left: Vec<QueryResult>, right: Vec<QueryResult>) -> Vec<QueryResult> {
    let mut map: HashMap<String, QueryResult> = HashMap::with_capacity(left.len() + right.len());

    for result in left.into_iter().chain(right.into_iter()) {
        match map.get_mut(&result.0) {
            Some(existing) => {
                existing.1.extend(result.1);

                existing.1.sort_by(|a, b| {
                    b.1
                        .cmp(&a.1)
                        .then_with(|| a.0.cmp(&b.0))
                });

                existing.1.truncate(TOP_K);
            }

            None => {
                map.insert(result.0.clone(), result);
            }
        }
    }

    map.into_values().collect()
}

fn divide_and_conquer_seq(queries: &'static [Sequence], database: &'static [Sequence]) -> Vec<QueryResult> {
    let query_size = queries.len();
    let database_size = database.len();

    if query_size.max(database_size) <= THRESHOLD {
        return process_leaf(queries, database);
    }

    // Preferentially split the database when it is larger than the threshold. 
    // Otherwise it splits the query set.
    let (left, right) = if database_size > THRESHOLD {
        let midpoint = database_size / 2;
        ((queries, &database[..midpoint]),(queries, &database[midpoint..]))
    } else {
        let midpoint = query_size / 2;
        ((&queries[..midpoint], database), (&queries[midpoint..], database))
    };

    let left_result = divide_and_conquer_seq(left.0, left.1);
    let right_result = divide_and_conquer_seq(right.0, right.1);

    combine(left_result, right_result)
}

fn seq_main(queries: &'static [Sequence], database: &'static [Sequence]) {
    let start = std::time::Instant::now();
    let results = divide_and_conquer_seq(&queries, &database);
    let elapsed = start.elapsed();

    println!("Results:  {}", results.len());
    println!("SEQ Time: {:.3} seconds", elapsed.as_secs_f64());

    println!();
}

#[spawnable]
fn divide_and_conquer(queries: &'static [Sequence], database: &'static [Sequence]) -> Vec<QueryResult> {
    let query_size = queries.len();
    let database_size = database.len();

    if query_size.max(database_size) <= THRESHOLD {
        return process_leaf(queries, database);
    }

    // Preferentially split the database when it is larger than the threshold. 
    // Otherwise it splits the query set.
    let (left, right) = if database_size > THRESHOLD {
        let midpoint = database_size / 2;
        ((queries, &database[..midpoint]),(queries, &database[midpoint..]))
    } else {
        let midpoint = query_size / 2;
        ((&queries[..midpoint], database), (&queries[midpoint..], database))
    };

    let left_result = divide_and_conquer(left.0, left.1);
    let right_result = divide_and_conquer(right.0, right.1);

    combine(left_result, right_result)
}

#[velvet_main]
fn velvet_main(queries: &'static [Sequence], database: &'static [Sequence]) {
    let start = std::time::Instant::now();

    let results = divide_and_conquer(&queries, &database);

    let elapsed = start.elapsed();

    println!("Results:  {}", results.len());
    println!("VELVET Time: {:.3} seconds", elapsed.as_secs_f64());

    println!();

    // TODO: only if CLArg asks for it
    dump(&results);
}
fn main() -> Result<(), Box<dyn Error>> {
    let queries: &'static [Sequence] = Box::leak(load_fasta("testdata/set02q.fasta")?.into_boxed_slice());
    let database: &'static [Sequence] = Box::leak(load_fasta("testdata/set02db.fasta")?.into_boxed_slice());

    println!("queries:  {}", queries.len());
    println!("database: {}", database.len());

    seq_main(queries, database);
    velvet_main(queries, database);
    
    Ok(())
}