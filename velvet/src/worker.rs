use std::{thread, sync::{Arc, atomic::{AtomicBool, Ordering}, Barrier}};
use super::{queue::{Identifiable, VelvetQueue, VelvetStealer}, VelvetRng};
#[cfg(feature = "stats")]
use super::RuntimeStats;
#[cfg(feature = "stats")]
use std::time::Duration;

pub struct VelvetWorker<T: Identifiable + Send + 'static>  {
    _id: usize,
    queue: Arc<VelvetQueue<T>>,
    pub stealers: Vec<VelvetStealer<T>>,
    done: Arc<AtomicBool>,
    barrier: Arc<Barrier>,
    sequence_nr: usize,
    rng: VelvetRng,
    steal: fn(&mut VelvetWorker<T>),
    handles: Option<Vec<thread::JoinHandle<()>>>,
    #[cfg(feature = "stats")]
    stats: RuntimeStats,
}
impl <T: Identifiable + Send + 'static> VelvetWorker <T> {
    fn new(id: usize, queue_size: usize, done: Arc<AtomicBool>, barrier: Arc<Barrier>, steal: fn(&mut VelvetWorker<T>)) -> Self {
        let queue = Arc::new(VelvetQueue::<T>::new(queue_size));
        let stealers = Vec::new();
        Self {
            _id: id,
            queue,
            stealers,
            done,
            barrier,
            sequence_nr: 0,
            rng: VelvetRng::new(),
            steal, 
            handles: None,
            #[cfg(feature = "stats")]
            stats: RuntimeStats::new(),
        }
    }

    /// Create num_workers-many workers, and move them into their own (pinned) thread
    /// Returns the root worker running on current thread, and a vector of join handles for the spawned threads
    /// TODO: parameterise pinning config
    pub fn prepare_workers(num_workers: usize, queue_size: usize, steal: fn(&mut VelvetWorker<T>)) -> Self {
        let mut workers = Vec::with_capacity(num_workers);
        let mut stealers = Vec::with_capacity(num_workers);
        let done = Arc::from(AtomicBool::new(false));
        let barrier = Arc::from(Barrier::new(num_workers));
        for id in 0..num_workers {
            workers.push(Self::new(id, queue_size, done.clone(), barrier.clone(), steal));
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
        let core_ids = core_affinity::get_core_ids().unwrap();
        for thread_nr in 1..num_workers {
            let id = core_ids[thread_nr];
            let mut worker = workers.pop().unwrap();
            joinhandles.push(thread::spawn(move || {
                // pin this thread to the given CPU core.
                let res = core_affinity::set_for_current(id);
                if !res {
                    eprintln!("Could not pin worker thread id {:?}, continuing without pinning...", id);
                }
                worker.wait();
                while !worker.done.load(Ordering::Relaxed) {
                    worker.steal();
                }
            }));
        }

        // MAKE ROOT WORKER
        let mut root_worker = workers.pop().unwrap();
        // set handles-field
        root_worker.handles = Some(joinhandles);
        // pin this thread to a single CPU core
        let id = core_ids[0];
        let res = core_affinity::set_for_current(id);
        if !res {
            eprintln!("Could not pin Root thread id {:?}, continuing without pinning...", id);
        }
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
        self.queue.pop(id)
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