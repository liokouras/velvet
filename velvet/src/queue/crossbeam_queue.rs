use std::{collections::HashMap, sync::{Arc, Mutex}};
use crossbeam_deque::{Stealer, Steal, Worker};

use super::Identifiable;

pub(crate) struct Queue<T: Identifiable> {
	queue: Worker<T>,
	stolen: Arc<Mutex<HashMap<usize, T>>>,
}
unsafe impl <T: Identifiable> Sync for Queue<T>{}

impl <T: Identifiable> Queue<T> {
    pub(crate) fn new(_capacity: usize) -> Self {
        Self {
            queue: Worker::new_lifo(),
            stolen: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub(crate) fn push(&self, frame: T) {
        self.queue.push(frame);
    }

    pub(crate) fn pop(&self, uid: usize) -> T {
        match self.queue.pop() {
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

    fn get_returnslot(&self, uid: usize) -> Option<T> {
		return self.stolen.lock().expect("failed to lock when receiving").remove(&uid);
	}
}

pub struct VelvetStealer<T: Identifiable> {
	stealer: Stealer<T>,
    stolen: Arc<Mutex<HashMap<usize, T>>>,
}

impl <T: Identifiable> VelvetStealer<T> {
    pub(crate) fn new(queue: Arc<Queue<T>>) -> Self {
        Self {
            stealer: queue.queue.stealer(),
            stolen: queue.stolen.clone(),
        }
    }

	// called by other threads
	pub fn steal(&self, trace: T) -> Option<T> {
        let stolen = self.stealer.steal();
        
		if let Steal::Success(job) = stolen {
			let id = job.get_id();
			self.stolen.lock().expect("failed to lock when stealing").insert(id, trace);
            return Some(job);
		}

        None
	}
}

impl <T: Identifiable> Clone for VelvetStealer<T> {
    fn clone(&self) -> Self {
        Self {
            stealer: self.stealer.clone(),
            stolen: self.stolen.clone(),
        }
    }
}