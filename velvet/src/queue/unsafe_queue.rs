#![allow(unsafe_op_in_unsafe_fn)] 
use std::{cell::UnsafeCell, mem::MaybeUninit, ptr, sync::{Arc, atomic::{AtomicBool, Ordering}}};

// FROM CROSSBEAM-DEQUE: A buffer that holds tasks in a worker queue.
struct Buffer<T> {
    ptr: *mut T, // pointer to the allocated memory
    cap: usize, // capacity of the buffer - always a power of two
}
impl<T> Buffer<T> {
    ///allocates a new buffer with the specified capacity.
    fn alloc(cap: usize) -> Self {
        debug_assert_eq!(cap, cap.next_power_of_two());

        let ptr = Box::into_raw(
            (0..cap)
                .map(|_| MaybeUninit::<T>::uninit())
                .collect::<Box<[_]>>(),
        )
        .cast::<T>();

        Self { ptr, cap }
    }

    /// deallocates the buffer
    unsafe fn dealloc(self) {
        drop(unsafe {
            Box::from_raw(ptr::slice_from_raw_parts_mut(
                self.ptr.cast::<MaybeUninit<T>>(),
                self.cap,
            ))
        });
    }

    /// returns a pointer to the item at the specified index
    unsafe fn at(&self, index: isize) -> *mut T {
        unsafe { self.ptr.offset(index) }
    }

    /// writes an item into the specified index
    unsafe fn write(&self, index: usize, item: MaybeUninit<T>) {
        unsafe { ptr::write_volatile(self.at(index as isize).cast::<MaybeUninit<T>>(), item) }
    }

    /// reads an item from the specified index
    unsafe fn read(&self, index: usize) -> MaybeUninit<T> {
        unsafe { ptr::read_volatile(self.at(index as isize).cast::<MaybeUninit<T>>()) }
    }
}
impl<T> Clone for Buffer<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for Buffer<T> {}

pub struct Queue<T> {
    buffer: UnsafeCell<Buffer<T>>,
    length: UnsafeCell<usize>,
    steal_idx: UnsafeCell<usize>,
    lock: AtomicBool,
}
unsafe impl<T> Send for Queue<T> {}
unsafe impl<T> Sync for Queue<T> {}
impl<T> Queue<T> {
    pub fn new(capacity: usize) -> Self {
        let buffer = Buffer::alloc(capacity);
        Self {
            buffer: UnsafeCell::new(buffer),
            lock: AtomicBool::new(false),
            steal_idx: UnsafeCell::new(0),
            length: UnsafeCell::new(0),
        }
    }

    unsafe fn grow(&self, new_cap: usize) {
        eprintln!("GROWING BUFFER! NEW CAP: {}", new_cap);
        // acquire lock
        while self.lock.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {}
        let buffer = *self.buffer.get();
        // allocate a new buffer and copy data from the old buffer to the new one.
        let new = Buffer::alloc(new_cap);
        let mut idx = 0;
        while idx != *self.length.get() {
            unsafe { ptr::copy_nonoverlapping(buffer.at(idx as isize), new.at(idx as isize), 1) }
            idx += 1;
        }
        *self.buffer.get() = new;
        // free the memory allocated by the buffer
        buffer.dealloc();
        // unlock
        self.lock.store(false, Ordering::Release);
    }

    // TTAS LOCK
    fn lock(&self) {
        loop {
            while self.lock.load(Ordering::Relaxed){}
            if let Ok(_) = self.lock.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed) {
                return;
            }
        }
    }

    /// SAFETY: only 1 thread calls this
    /// writes to buffer and increments idx; it's acceptable if stealers see old (lower) idx value;
    /// worst case they see idx = 0 or idx = steal_idx, in which case they return None
    pub fn push(&self, frame: T) {
        unsafe {
            // get buffer and current index of empty slot
            let mut buffer = &*self.buffer.get();
            let cap = buffer.cap;
            let idx = &mut *self.length.get();

            // check if the buffer is full and if so, double its capacity
            if *idx >= cap {
                self.grow(2*cap);
                buffer = &*self.buffer.get();
            }

            // write the frame to the buffer
            buffer.write(*idx, MaybeUninit::new(frame));
            
            // increment index of empty slot
            *idx += 1;
        }
    }

    /// SAFETY: only 1 thread calls this
    /// decrements idx, which matters in case idx and steal_idx are 'close' since a stealer
    /// could be trying to steal the same frame being popped.
    /// thus we lock when we update idx and steal_idx to ensure popped frame cannot be stolen
    /// then we pop
    pub fn pop(&self, _: usize) -> T {
        unsafe {
            // get current index of empty slot and check queue isn't empty
            let idx = &mut *self.length.get();
            if *idx == 0 {
                panic!("Popped on empty queue");
            }
            
            // acquire lock
            self.lock();
            // check if steal_idx == len, and if so, decrement steal_idx
            let s_idx = &mut *self.steal_idx.get();
            if *idx == *s_idx {
                *s_idx -= 1;
            }
            // decrement index of empty slot
            *idx -= 1;
            // unlock
            self.lock.store(false, Ordering::Release);

            // get the frame from the buffer
            let buffer = &*self.buffer.get();
            buffer.read(*idx).assume_init()
        }
    }

    pub fn steal(&self, trace: T) -> Option<T> {
        unsafe {
            // get current index of empty slot and steal slot
            let idx = &*self.length.get();
            let s_idx = &*self.steal_idx.get();
            // check queue isn't empty
            if *idx == 0 || *idx <= *s_idx {
                return None;
            }
        }

        // queue is not empty, acquire lock
        self.lock();

        let frame;
        unsafe {
            // re-read indices, re-check queue isn't empty
            let s_idx = &mut *self.steal_idx.get();
            let idx = &*self.length.get();
            if *idx == 0 || *idx <= *s_idx {
                // unlock
                self.lock.store(false, Ordering::Release);
                return None;
            }

            // get the frame from the buffer, replace with the trace
            let buffer = &*self.buffer.get();
            frame = Some(buffer.read(*s_idx));
            buffer.write(*s_idx, MaybeUninit::new(trace));

            // increment index of steal slot
            *s_idx += 1;
        }
        // unlock
        self.lock.store(false, Ordering::Release);

        frame.map(|t| unsafe { t.assume_init() })
    }
}
impl<T> Drop for Queue<T> {
    fn drop(&mut self) {
        unsafe {
            let len = *self.length.get();
            let buffer = &mut *self.buffer.get();

            // drop all frames in the buffer
            let mut idx = 0;
            while idx != len {
                buffer.at(idx as isize).drop_in_place();
                idx += 1;
            }

            // free the memory allocated by the buffer
            buffer.dealloc();
        }
    }
}

pub struct Stealer<T>  { queue: Arc<Queue<T>>, }
impl <T> Stealer<T>  {
    pub(crate) fn new(queue: Arc<Queue<T>>) -> Self {
        Self { queue }
    }
    // interface to steal
    pub fn steal(&self, trace: T) -> Option<T> {
        self.queue.steal(trace)
    }
}
impl <T> Clone for Stealer<T> {
    fn clone(&self) -> Self {
        Self { queue: self.queue.clone() }
    }
}