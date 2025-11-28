pub mod prelude;
mod codegen;
mod queue;
mod worker;

pub use codegen::generate;
pub use worker::VelvetWorker;
pub use queue::{velvet_get_queue_name, Identifiable};
pub use velvet_macros::{spawnable, velvet_main};

// simple XORSHIFT pseudo-random number generator
use std::{cell::Cell, collections::hash_map::RandomState, hash::{BuildHasher, Hasher}};
pub(crate) struct VelvetRng {
	state: Cell<u32>,
}
impl VelvetRng {
	fn new() -> Self {
        // need any non-zero starting state
        let mut state = 0;
        while state == 0 {
            let mut hasher = RandomState::new().build_hasher();
            hasher.write_u32(state);
            state = hasher.finish() as u32;
        }

        VelvetRng {
            state: Cell::new(state),
        }
    }

	fn next(&self) -> u32 {
	    let mut x = self.state.get();
	    x ^= x << 13;
	    x ^= x >> 17;
	    x ^= x << 5;
	    self.state.set(x);
        x
	}

    fn get_random(&self, n: usize) -> usize {
        (self.next() % n as u32) as usize
    }
}

// struct to keep track of runtime statistics
#[cfg(feature = "stats")]
use std::time::Duration;
#[cfg(feature = "stats")]
#[allow(dead_code)]
pub struct RuntimeStats {
    total_steal_attempts: usize,
    successful_steals: usize,
    attempts_before_first_success: usize,
    spawns: usize,
    work_time: Duration,
    other_time: Duration,
    steal_setup_time: Duration,
    steal_waiting: Duration,
    pop_waiting: Duration,
    push_waiting: Duration,
    total_stolen_jobs: usize,
    sync_loop_iters: usize,
    spawn_other: usize, // for BH: there are 2 spawnable funcs
    sync_loop_iters_other: usize,
    stolen_jobs_other: usize,
    push_waiting_other: Duration,
    pop_waiting_other: Duration,

}
#[cfg(feature = "stats")]
#[allow(dead_code)]
impl RuntimeStats {
    fn new() -> RuntimeStats{
        RuntimeStats {
            total_steal_attempts: 0,
            successful_steals: 0,
            attempts_before_first_success: 0,
            spawns: 0,
            work_time: Duration::default(),
            steal_setup_time: Duration::default(),
            steal_waiting: Duration::default(),
            pop_waiting: Duration::default(),
            push_waiting: Duration::default(),
            other_time: Duration::default(),
            total_stolen_jobs: 0,
            sync_loop_iters: 0,
            spawn_other: 0,
            sync_loop_iters_other: 0,
            stolen_jobs_other: 0,
            push_waiting_other: Duration::default(),
            pop_waiting_other: Duration::default(),
        }
    }

    fn add_steal_attempts(&mut self, n: usize) {
        self.total_steal_attempts += n;
    }

    fn add_successful_steals(&mut self, n: usize) {
        if self.successful_steals == 0 {
            self.attempts_before_first_success = self.total_steal_attempts;
        }
        self.successful_steals += n;
    }

    fn add_steal_setup_time(&mut self, d: Duration) {
        self.steal_setup_time += d;
    }

    fn add_steal_waittime(&mut self, d: Duration) {
        self.steal_waiting += d;
    }

    fn add_spawns(&mut self, n: usize) {
        self.spawns += n;
    }

    fn add_pop_waittime(&mut self, d: Duration) {
        self.pop_waiting += d;
    }

    fn add_push_waittime(&mut self, d: Duration) {
        self.push_waiting += d;
    }

    fn add_work_time(&mut self, d: Duration) {
        self.work_time += d;
    }

    fn add_other_time(&mut self, d: Duration) {
        self.other_time += d;
    }

    fn add_stolen_jobs(&mut self, n: usize) {
        self.total_stolen_jobs += n;
    }

    pub fn add_sync_loop_iters(&mut self, n: usize) {
        self.sync_loop_iters += n;
    }

    pub fn add_push_waittime_other(&mut self, d: Duration) {
        self.push_waiting_other += d;
    }

    pub fn add_pop_waittime_other(&mut self, d: Duration) {
        self.pop_waiting_other += d;
    }

    pub fn add_spawns_other(&mut self, n: usize) {
        self.spawn_other += n;
    }

    pub fn add_stolen_jobs_other(&mut self, n: usize) {
        self.stolen_jobs_other += n;
    }

    pub fn add_sync_loop_iters_other(&mut self, n: usize) {
        self.sync_loop_iters += n;
    }


    fn dump(&self, id: usize) {
        eprintln!("{},{},{},{},{:?},{:?},{:?},{:?},{:?},{:?},{:?},{:?},{},{},{},{},{},{}",
                id,
                self.total_steal_attempts,
                self.successful_steals,
                self.attempts_before_first_success,
                self.work_time,
                self.steal_setup_time,
                self.steal_waiting,
                self.pop_waiting,
                self.pop_waiting_other,
                self.push_waiting,
                self.push_waiting_other,
                self.other_time,
                self.spawns,
                self.spawn_other,
                self.total_stolen_jobs,
                self.stolen_jobs_other,
                self.sync_loop_iters,
                self.sync_loop_iters_other,
            );
    }
}

// indirect way to get the number of workers...
pub fn velvet_get_num_workers() -> usize {
    match std::env::var("VELVET_WORKERS") {
        Ok(string_value) => string_value.parse::<usize>().expect("make sure VELVET_WORKERS env var is a positive integer"),
        _ => std::thread::available_parallelism().unwrap().into(),
    }
}

/*
#[cfg(test)]
mod test {
    use std::sync::Arc;

    use crate::queue::*;
    use super::VelvetFrame;
    use super::queue::VelvetQueue;

    // combine queue + frame with basic frame type
    #[test]
    fn enq_deq_frame_basic(){
        enum Frame {
            FuncInput(VelvetFrame::FrameData<usize>),
        }

        let queue = VelvetQueue::<Frame>::new(64);
        let frame1 = Frame::FuncInput(VelvetFrame::put(0));
        let frame2 = Frame::FuncInput(VelvetFrame::put(1));
        let frame3 = Frame::FuncInput(VelvetFrame::put(2));

        queue.push(frame1);
        queue.push(frame2);
        queue.push(frame3);

        if let Pop::Job(Frame::FuncInput(framedata)) = queue.pop() {
            assert_eq!(VelvetFrame::take(framedata), 2);
        } else {
            panic!("popped wrong item from queue");
        }

        if let Pop::Job(Frame::FuncInput(framedata)) = queue.pop() {
            assert_eq!(VelvetFrame::take(framedata), 1);
        } else {
            panic!("popped wrong item from queue");
        }

        if let Pop::Job(Frame::FuncInput(framedata)) = queue.pop() {
            assert_eq!(VelvetFrame::take(framedata), 0);
        } else {
            panic!("popped wrong item from queue");
        }

        if let Pop::Empty = queue.pop(){
        } else {
            panic!("queue should be empty");
        }
    }
    
    // combine queue + frame with reference frame type
    #[test]
    fn enq_deq_frame_ref(){
        enum Frame<'a> {
            FuncInput(VelvetFrame::FrameData<&'a[f64]>),
        }

        let slice = &[0.0, 1.1, 2.2, 3.3];

        let queue = VelvetQueue::<Frame>::new(64);
        let frame1 = Frame::FuncInput(VelvetFrame::put(slice));
        let frame2 = Frame::FuncInput(VelvetFrame::put(&slice[1..]));
        let frame3 = Frame::FuncInput(VelvetFrame::put(&slice[2..3]));

        queue.push(frame1);
        queue.push(frame2);
        queue.push(frame3);

        if let Pop::Job(Frame::FuncInput(framedata)) = queue.pop() {
            assert_eq!(VelvetFrame::take(framedata), &slice[2..3]);
        } else {
            panic!("popped wrong item from queue");
        }

        if let Pop::Job(Frame::FuncInput(framedata)) = queue.pop() {
            assert_eq!(VelvetFrame::take(framedata), &slice[1..]);
        } else {
            panic!("popped wrong item from queue");
        }

        if let Pop::Job(Frame::FuncInput(framedata)) = queue.pop() {
            assert_eq!(VelvetFrame::take(framedata), slice);
        } else {
            panic!("popped wrong item from queue");
        }

        if let Pop::Empty = queue.pop(){
        } else {
            panic!("queue should be empty");
        }
    }

    // combine queue + frame: multithreaded
    #[test]
    fn queue_multithreaded() {
        use std::{sync::{atomic::{AtomicBool, Ordering}, Barrier}, thread};

        enum Frame<'a> {
            FuncInput(VelvetFrame::FrameData<(usize, &'a[f64])>),
            FuncOutput(VelvetFrame::FrameData<f64>)
        }
        let slice = &[0.0,1.1,2.2,3.3,4.4,5.5,6.6,7.7,8.8,9.9,10.1,11.11,12.12,13.13,14.14,15.15,16.16,17.17,18.18,19.19,20.2,21.21,22.22,23.23,24.24,25.25,26.26,27.27,28.28,29.29,30.30,31.31];


        let problem_size = 32;
        let num_threads = 8;
        // queue
        let q: Arc<VelvetQueue<Frame>> = Arc::new(VelvetQueue::new(problem_size));
        // barrier
        let barrier = Arc::new(Barrier::new(num_threads));
        // signal
        let signal = Arc::new(AtomicBool::new(true));
        // spawn worker threads (thieves)
        let mut handles = Vec::new();
        for _thread_id in 0..num_threads-1 {
            let barrier = barrier.clone();
            let stealer = VelvetStealer::new(q.clone());
            let signal = signal.clone();
            handles.push(thread::spawn(move || {
                barrier.wait();
                // steal work
                while signal.load(Ordering::Relaxed) {
                    if let Some((id, frame)) = stealer.steal() {
                        // println!("thread id {} got frame at queue index {}", _thread_id, id);
                        match frame {
                            Frame::FuncInput(data) => {
                                let (idx, input) = VelvetFrame::take(data);
                                let result = input[idx] * 2.;
                                let output = Frame::FuncOutput(VelvetFrame::put(result));
                                stealer.return_stolen(id, output);
                            },
                            Frame::FuncOutput(_) => panic!("stolen frame already has output"),
                        }
                    }
                }
            }));
        }

        // root worker
        barrier.wait();
        for i in 0..problem_size {
            q.push(Frame::FuncInput(VelvetFrame::put((0, &slice[i..]))));
        }

        for i in (0..problem_size).rev() {
            match q.pop() {
                Pop::Empty => panic!("queue should not be empty at idx {}", i),
                Pop::Job(frame) => {
                    println!("worker popped a local job at index {}", i);
                    match frame {
                        Frame::FuncInput(data) => {
                            let (idx, input) = VelvetFrame::take(data);
                            assert_eq!(0, idx);
                            assert_eq!(&slice[i..], input);
                        },
                        Frame::FuncOutput(_) => panic!("output frame listed as pop::Job"),
                    }
                },
                Pop::StolenDone(frame) => {
                    match frame {
                        Frame::FuncInput(_) => panic!("input frame listed as pop::StolenDone"),
                        Frame::FuncOutput(data) => {
                            let output = VelvetFrame::take(data);
                            println!("got output {}", output);
                            assert_eq!(output, slice[i] * 2.)
                        },
                    }
                },
                Pop::StolenInProgress => {
                    loop {
                        match q.pop() {
                            Pop::StolenDone(Frame::FuncOutput(data)) => { 
                                let output = VelvetFrame::take(data);
                                assert_eq!(output, slice[i] * 2.);
                                break;
                            },
                            Pop::Job(_) => panic!("Got Pop::Job after StolenInProgress"),
                            Pop::Empty => panic!("Got Pop::Empty after StolenInProgress"),
                            _ => (),
                        }
                    }
                }
            }
        }
       
       // shutdown
        signal.store(false, Ordering::Relaxed);
        for handle in handles {
            let _ = handle.join();
        }
    }
}
    */