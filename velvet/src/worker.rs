use std::{any::Any, panic::{self, AssertUnwindSafe}, thread, sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}, Barrier}};
use super::{queue::{Identifiable, VelvetQueue, VelvetStealer}, VelvetRng};
#[cfg(feature = "stats")]
use super::RuntimeStats;
#[cfg(feature = "stats")]
use std::time::Duration;

/*
    Panics in tasks. A task can panic on any worker, but only the root worker (the thread that called velvet_main) can
    report it to the user, and every worker waiting for a result of the panicked task must stop waiting.
    So a panic *aborts* the pool:
    - a thief runs a stolen task under catch_unwind (in the generated steal function); on a panic it records the
      payload here and sets `aborted`, releases the task's result slot, and unwinds its own stack (abort_unwind);
    - every worker checks `aborted` when it syncs (and while waiting for a stolen result), and unwinds its stack;
    - the root re-raises the recorded payload, so velvet_main panics with the original panic;
    - worker threads catch the unwinding at the bottom of their steal loop, and stop.
    (A panic on the root thread itself unwinds velvet_main directly; the root worker's Drop then aborts the pool.)
*/
struct AbortState {
    aborted: AtomicBool,
    payload: Mutex<Option<Box<dyn Any + Send>>>, // the first panic's payload, re-raised by the root
}

// payload for unwinding the stacks of the other workers after a panic (resume_unwind: not reported by the panic hook)
struct VelvetAbort;

pub struct VelvetWorker<T: Identifiable + Send + 'static>  {
    _id: usize,
    queue: Arc<VelvetQueue<T>>,
    pub stealers: Vec<VelvetStealer<T>>,
    done: Arc<AtomicBool>,
    abort: Arc<AbortState>,
    barrier: Arc<Barrier>,
    sequence_nr: usize,
    rng: VelvetRng,
    steal: fn(&mut VelvetWorker<T>),
    handles: Option<Vec<thread::JoinHandle<()>>>,
    #[cfg(feature = "stats")]
    stats: RuntimeStats,
}
impl <T: Identifiable + Send + 'static> VelvetWorker <T> {
    fn new(id: usize, queue_size: usize, done: Arc<AtomicBool>, abort: Arc<AbortState>, barrier: Arc<Barrier>, steal: fn(&mut VelvetWorker<T>)) -> Self {
        let queue = Arc::new(VelvetQueue::<T>::new(queue_size));
        let stealers = Vec::new();
        Self {
            _id: id,
            queue,
            stealers,
            done,
            abort,
            barrier,
            sequence_nr: 0,
            rng: VelvetRng::new(),
            steal, 
            handles: None,
            #[cfg(feature = "stats")]
            stats: RuntimeStats::new(),
        }
    }

    /// Create num_workers-many workers, and move them into their own thread
    /// Returns the root worker running on current thread, holding the join handles of the spawned threads
    /// (Velvet does not pin threads to cores; that can be done from outside Velvet, e.g. with taskset or numactl)
    pub fn prepare_workers(num_workers: usize, queue_size: usize, steal: fn(&mut VelvetWorker<T>)) -> Self {
        let mut workers = Vec::with_capacity(num_workers);
        let mut stealers = Vec::with_capacity(num_workers);
        let done = Arc::from(AtomicBool::new(false));
        let abort = Arc::new(AbortState { aborted: AtomicBool::new(false), payload: Mutex::new(None) });
        let barrier = Arc::from(Barrier::new(num_workers));
        for id in 0..num_workers {
            workers.push(Self::new(id, queue_size, done.clone(), abort.clone(), barrier.clone(), steal));
        }
        for worker in &workers {
            let stealer = worker.get_stealer();
            stealers.push(stealer);
        }
        for i in 0..num_workers {
            let mut stealers_vec = stealers.clone();
            stealers_vec.remove(i);
            workers[i].add_stealers(stealers_vec);
        }

        // MOVE WORKERS TO THREADS
        let mut joinhandles = Vec::with_capacity(num_workers);
        for _ in 1..num_workers {
            let mut worker = workers.pop().unwrap();
            joinhandles.push(thread::spawn(move || {
                worker.wait();
                // a panic in a task this worker runs (or the abort after a panic elsewhere) ends up here
                let outcome = panic::catch_unwind(AssertUnwindSafe(|| {
                    while !worker.done.load(Ordering::Relaxed) && !worker.is_aborted() {
                        worker.steal();
                    }
                }));
                if let Err(payload) = outcome {
                    worker.record_panic(payload);
                }
            }));
        }

        // MAKE ROOT WORKER
        let mut root_worker = workers.pop().unwrap();
        // set handles-field
        root_worker.handles = Some(joinhandles);
        root_worker
    }

    fn get_stealer(&self) -> VelvetStealer<T> {
        VelvetStealer::new(self.queue.clone())
    }

    fn add_stealers(&mut self, stealers: Vec<VelvetStealer<T>>){
        self.stealers = stealers;
    }

    pub fn wait(&self) {
        self.barrier.wait();
    }

    pub fn set_done(&self){
        self.done.store(true, Ordering::Relaxed);
    }

    #[inline(always)]
    pub fn get_seq(&mut self) -> usize {
        let seq = self.sequence_nr;
        self.sequence_nr += 1;
        seq
    }

    #[inline(always)]
    pub fn spawn (&self, frame: T) {
        self.queue.push(frame);
    }

    #[inline(always)]
    pub fn sync(&self, id: usize) -> T {
        if self.is_aborted() { self.abort_unwind(); }
        self.queue.pop(id)
    }

    /// whether a task in this pool has panicked (see AbortState)
    ///
    /// Relaxed suffices: the flag is only a "stop now" hint, and correctness does not depend on its ordering.
    /// - a thief sets it *before* releasing the result slot of the panicked task (a mutex), so an owner that
    ///   acquires that slot is guaranteed to see it, through the mutex's own synchronisation;
    /// - the panic payload is handed over under its own mutex.
    /// Elsewhere (sync, the idle steal loop) seeing the flag a little late only delays stopping.
    /// Being written at most once, the flag's cache line stays shared by all cores, so this load is a cache hit.
    #[inline(always)]
    pub fn is_aborted(&self) -> bool {
        self.abort.aborted.load(Ordering::Relaxed)
    }

    /// records that running a task panicked, and aborts the pool. The first real panic's payload is kept,
    /// for the root to re-raise; the abort-payloads of workers unwinding afterwards are not.
    pub fn record_panic(&self, payload: Box<dyn Any + Send>) {
        if !payload.is::<VelvetAbort>() {
            let mut first = self.abort.payload.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            if first.is_none() { *first = Some(payload); }
        }
        self.abort.aborted.store(true, Ordering::Release);
    }

    /// unwinds this worker's stack because a task in the pool panicked: the root re-raises the original panic
    /// (so velvet_main panics with it); other workers unwind quietly (their threads then stop)
    pub fn abort_unwind(&self) -> ! {
        if self.handles.is_some() {
            let payload = self.abort.payload.lock().unwrap_or_else(|poisoned| poisoned.into_inner()).take();
            match payload {
                Some(payload) => panic::resume_unwind(payload),
                None => panic!("velvet: a task panicked"),
            }
        }
        panic::resume_unwind(Box::new(VelvetAbort))
    }

    #[inline(always)]
    pub fn get_random(&self, range: usize) -> usize {
        self.rng.get_random(range)
    }
    
    pub fn steal(&mut self) {
        (self.steal)(self);
    }

    #[cfg(feature = "stats")]
    pub fn dump_stats(&self) {
        self.stats.dump(self._id);
    }

    #[cfg(feature = "stats")]
    pub fn add_steal_attempts(&mut self, n: usize) {
        self.stats.add_steal_attempts(n);
    }

    #[cfg(feature = "stats")]
    pub fn add_successful_steals(&mut self, n: usize) {
        self.stats.add_successful_steals(n);
    }

    #[cfg(feature = "stats")]
    pub fn add_spawns(&mut self, n: usize) {
        self.stats.add_spawns(n);
    }
    
    #[cfg(feature = "stats")]
    pub fn add_steal_setup_time(&mut self, d: Duration) {
        self.stats.add_steal_setup_time(d);
    }

    #[cfg(feature = "stats")]
    pub fn add_steal_waittime(&mut self, d: Duration) {
        self.stats.add_steal_waittime(d);
    }

    #[cfg(feature = "stats")]
    pub fn add_pop_waittime(&mut self, d: Duration) {
        self.stats.add_pop_waittime(d);
    }

    #[cfg(feature = "stats")]
    pub fn add_push_waittime(&mut self, d: Duration) {
        self.stats.add_push_waittime(d);
    }

    #[cfg(feature = "stats")]
    pub fn add_work_time(&mut self, d: Duration) {
        self.stats.add_work_time(d);
    }

    #[cfg(feature = "stats")]
    pub fn add_other_time(&mut self, d: Duration) {
        self.stats.add_other_time(d);
    }

    #[cfg(feature = "stats")]
    pub fn add_stolen_jobs(&mut self, n: usize) {
        self.stats.add_stolen_jobs(n);
    }

    #[cfg(feature = "stats")]
    pub fn add_sync_loop_iters(&mut self, n: usize) {
        self.stats.add_sync_loop_iters(n);
    }

    #[cfg(feature = "stats")]
    pub fn add_stolen_jobs_other(&mut self, n: usize) {
        self.stats.add_stolen_jobs_other(n);
    }

    #[cfg(feature = "stats")]
    pub fn add_sync_loop_iters_other(&mut self, n: usize) {
        self.stats.add_sync_loop_iters_other(n);
    }

    #[cfg(feature = "stats")]
    pub fn add_push_waittime_other(&mut self, d: Duration) {
        self.stats.add_push_waittime_other(d);
    }

    #[cfg(feature = "stats")]
    pub fn add_pop_waittime_other(&mut self, d: Duration) {
        self.stats.add_pop_waittime_other(d);
    }

    #[cfg(feature = "stats")]
    pub fn add_spawns_other(&mut self, n: usize) {
        self.stats.add_spawns_other(n);
    }
}

impl <T: Identifiable + Send + 'static> Drop for VelvetWorker<T> {
    fn drop(&mut self) {
        // check if i am the root
        if let Some(handles) = self.handles.take() {
            // the root is unwinding because of a panic: stop the other workers at their next sync
            if thread::panicking() {
                self.abort.aborted.store(true, Ordering::Release);
            }
            self.set_done();
            for handle in handles {
                let _ = handle.join();
            }
        }

        #[cfg(feature = "stats")]
        self.dump_stats();
    }
}

/*
#[cfg(test)]
mod test_worker {
    use super::*;
    use crate::VelvetFrame;
    enum Frame {
        FuncInput(VelvetFrame::FrameData<(usize, &'static[f64])>),
        FuncOutput(VelvetFrame::FrameData<f64>),
    }
    fn steal(worker: &VelvetWorker<Frame>) {
        let stealers = &worker.stealers;
        let len = stealers.len();
        let mut n = worker.get_random(len);
        for _ in 0..len {
            let maybe_job = stealers[n].steal();
            if let Some((pos,frame)) = maybe_job {
                match frame {
                    Frame::FuncInput(framedata) => {
                        let (idx, input) = VelvetFrame::take(framedata);
                        let result = input[idx] * 2.5;
                        let output = Frame::FuncOutput(VelvetFrame::put(result));
                        stealers[n].return_stolen(pos, output);
                    }
                    Frame::FuncOutput(_) => panic!("stolen frame already has output"),
                }
            }
            n = (n + 1) % len;
        }
    }

    #[test]
    fn worker_multithreaded() {
        let num_workers = 8;
        let problem_size = 32;

        let slice = &[0.0,1.1,2.2,3.3,4.4,5.5,6.6,7.7,8.8,9.9,10.1,11.11,12.12,13.13,14.14,15.15,16.16,17.17,18.18,19.19,20.2,21.21,22.22,23.23,24.24,25.25,26.26,27.27,28.28,29.29,30.30,31.31];

        let (root_worker, join_handles) = VelvetWorker::<Frame>::prepare_workers(num_workers, problem_size, steal);

        root_worker.wait();
        // spawn
        for i in 0..problem_size {
            root_worker.spawn(Frame::FuncInput(VelvetFrame::put((0, &slice[i..]))));
        }
        //sync
        for i in (0..problem_size).rev() {
            let mut sync_result = root_worker.sync();
            loop {
                match sync_result {
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
                        break;
                    },
                    Pop::StolenDone(frame) => {
                        match frame {
                            Frame::FuncInput(_) => panic!("input frame listed as pop::StolenDone"),
                            Frame::FuncOutput(data) => {
                                let output = VelvetFrame::take(data);
                                println!("got output {}", output);
                                assert_eq!(output, slice[i] * 2.5)
                            },
                        }
                        break;
                    },
                    Pop::StolenInProgress => {
                        root_worker.steal();
                        sync_result = root_worker.sync();
                    }
                }
            }
        }
        
        // shutdowm
        root_worker.set_done();
        for handle in join_handles {
            let _ = handle.join();
        }
    }

}*/