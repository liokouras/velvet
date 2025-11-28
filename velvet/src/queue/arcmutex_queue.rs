use std::{collections::{HashMap, VecDeque}, sync::{Arc, Mutex}};
use super::Identifiable;
pub(crate) struct Queue<T: Identifiable> {
	queue: Arc<Mutex<VecDeque<T>>>,
	stolen: Arc<Mutex<HashMap<usize, T>>>,
}

impl <T: Identifiable> Queue<T> {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            queue: Arc::new(Mutex::new(VecDeque::with_capacity(capacity))),
            stolen: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub(crate) fn push(&self, frame: T) {
        self.queue.lock().unwrap().push_back(frame);
    }

    pub(crate) fn pop(&self, uid: usize) -> T {
        match self.queue.lock().unwrap().pop_back() {
            Some(frame) => return frame,
            None => {
                loop {
                    if let Some(returnslot) = self.get_returnslot(uid) {
                        return returnslot;
                    }
                }
            }
        }
    }

	pub(crate) fn steal(&self, trace: T) -> Option<T> {
        let stolen = self.queue.lock().unwrap().pop_front();
        
		if let Some(ref job) = stolen {
			let id = job.get_id();
			self.stolen.lock().expect("failed to lock when stealing").insert(id, trace);
		}

        stolen
	}
	
	fn get_returnslot(&self, uid: usize) -> Option<T> {
		return self.stolen.lock().expect("failed to lock when receiving").remove(&uid);
	}
}

pub struct Stealer<T:Identifiable>  { queue: Arc<Queue<T>>, }
impl <T: Identifiable> Stealer<T>  {
    pub(crate) fn new(queue: Arc<Queue<T>>) -> Self {
        Self { queue }
    }
    // interface to steal
    pub fn steal(&self, trace: T) -> Option<T> {
        self.queue.steal(trace)
    }
}
impl <T: Identifiable> Clone for Stealer<T> {
    fn clone(&self) -> Self {
        Self { queue: self.queue.clone() }
    }
}

/*
#[cfg(test)]
mod test_arcmutex {
    use super::*;
    use crate::queue::VelvetStealer;

    #[derive(Debug, PartialEq)]
    struct Frame {
        content: usize,
        id: usize,
    }
    impl Identifiable for Frame {
        fn get_id(&self) -> usize {
            self.id
        }
    }

    // test the queue: push & pop
    #[test]
    fn queue_pp() {
        let q: Queue<Frame, Frame> = Queue::new(4);

        q.push(Frame { content: 0, id: 0 });
        q.push(Frame { content: 1, id: 1 });
        q.push(Frame { content: 2, id: 2 });
        
        assert_eq!(q.pop(2), Frame { content: 2, id: 2 });
        assert_eq!(q.pop(1), Frame { content: 1, id: 1 });

        q.push(Frame { content: 3, id: 3 });
        assert_eq!(q.pop(3), Frame { content: 3, id: 3 });
        assert_eq!(q.pop(0), Frame { content: 0, id: 0 });
        
        q.push(Frame { content: 0, id: 0 });
        q.push(Frame { content: 1, id: 1 });
        q.push(Frame { content: 2, id: 2 });
        q.push(Frame { content: 3, id: 3 });
        q.push(Frame { content: 4, id: 4 }); // check buffer is grown (no panic)
    }

    // test the queue: steal
    #[test]
    fn queue_steal() {
        let q = Arc::new(Queue::<Frame, Frame>::new(4));
        let s = VelvetStealer { queue: q.clone() };

        assert_eq!(s.steal(Frame { content: 0, id: 0 }), None);

        q.push(Frame { content: 1, id: 1 });
        q.push(Frame { content: 2, id: 2 });
        q.push(Frame { content: 3, id: 3 });
        
        assert_eq!(s.steal(Frame { content: 0, id: 0 }), Some(Frame { content: 1, id: 1 }));
        assert_eq!(s.steal(Frame { content: 4, id: 4 }), Some(Frame { content: 2, id: 2 }));
        assert_eq!(q.pop(3), Frame { content: 3, id: 3 });
        assert_eq!(q.pop(1), Frame { content: 0, id: 0 });
        assert_eq!(q.pop(2), Frame { content: 4, id: 4 });
    }

    // test the queue: multithreaded
    // #[test]
    // fn queue_multithreaded() {
    //     use std::{sync::{atomic::{AtomicBool, Ordering}, Barrier}, thread};
    //     let problem_size = 4096;
    //     let num_threads = 12;
    //     // queue
    //     let q: Arc<Queue<usize>> = Arc::new(Queue::new(problem_size));
    //     // barrier
    //     let barrier = Arc::new(Barrier::new(num_threads));
    //     // signal
    //     let signal = Arc::new(AtomicBool::new(true));
    //     // spawn worker threads (thieves)
    //     let mut handles = Vec::new();
    //     for _thread_id in 0..num_threads-1 {
    //         let barrier = barrier.clone();
    //         let stealer = Stealer { queue: q.clone() };
    //         let signal = signal.clone();
    //         handles.push(thread::spawn(move || {
    //             barrier.wait();
    //             // steal work
    //             while signal.load(Ordering::Relaxed) {
    //                 if let Some((id, task)) = stealer.steal() {
    //                     // println!("thread id {} got task at queue index {}", _thread_id, id);
    //                     stealer.return_stolen(id, task*10);
    //                 }
    //             }
    //         }));
    //     }

    //     // root worker
    //     barrier.wait();
    //     for i in 0..problem_size {
    //         q.push(i);
    //     }

    //     for expected in (0..problem_size).rev() {
    //         match q.pop() {
    //             Pop::Empty => panic!("queue should not be empty at idx {}", expected),
    //             Pop::Job(i) => {
    //                 // println!("worker popped a local job at index {}", i);
    //                 assert_eq!(i, expected)
    //             },
    //             Pop::StolenDone(i) => assert_eq!(i, expected*10),
    //             Pop::StolenInProgress => {
    //                 loop {
    //                     match q.pop() {
    //                         Pop::StolenDone(i) => { 
    //                             assert_eq!(i, expected*10);
    //                             break;
    //                         },
    //                         _ => (),
    //                     }
    //                 }
    //             }
    //         }
    //     }
       
    //    // shutdown
    //     signal.store(false, Ordering::Relaxed);
    //     for handle in handles {
    //         let _ = handle.join();
    //     }
    // }
}*/