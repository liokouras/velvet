/// CAN ADD MORE QUEUE IMPLEMENTATIONS; 
/// just need to implement the following APIs:
///     fn push(&self, frame: T)
///     fn pop(&self, uid: usize) -> T
///     fn steal(&self, trace: T) -> Option<T>
/// and add name to velvet_get_queue_name() below
pub fn velvet_get_queue_name() -> String {
    #[cfg(feature = "safe")]
    return "safe".to_string();
    #[cfg(feature = "unsafe")]
    return "unsafe".to_string();
    #[cfg(feature = "crossbeam")]
    return "crossbeam".to_string();
}

#[cfg(feature = "safe")]
mod arcmutex_queue;
#[cfg(feature = "safe")]
pub(crate) use arcmutex_queue::Queue as VelvetQueue;
#[cfg(feature = "safe")]
pub(crate) use arcmutex_queue::Stealer as VelvetStealer;

#[cfg(feature = "crossbeam")]
mod crossbeam_queue;
#[cfg(feature = "crossbeam")]
pub(crate) use crossbeam_queue::Queue as VelvetQueue;
#[cfg(feature = "crossbeam")]
pub(crate) use crossbeam_queue::VelvetStealer;

#[cfg(feature = "unsafe")]
mod unsafe_queue;
#[cfg(feature = "unsafe")]
pub(crate) use unsafe_queue::Queue as VelvetQueue;
#[cfg(feature = "unsafe")]
pub(crate) use unsafe_queue::Stealer as VelvetStealer;

pub trait Identifiable {
    fn get_id(&self) -> usize;
}