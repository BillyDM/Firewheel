use core::sync::atomic::{AtomicBool, Ordering};
use ringbuf::traits::{Consumer, Observer, Producer, Split};

#[cfg(not(feature = "std"))]
use bevy_platform::prelude::String;

use crate::collector::ArcGc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "bevy_reflect", derive(bevy_reflect::Reflect))]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct RealtimeLoggerConfig {
    /// The capacity of each message slot. This determines the maximum length a
    /// single log message can be.
    ///
    /// It is highly recommended to have this be at least `128`.
    ///
    /// By default this is set to `128`.
    pub max_message_length: usize,

    /// The number of slots available. This determines how many log messages
    /// can be queued at once.
    ///
    /// By default this is set to `32`.
    pub num_slots: usize,
}

impl Default for RealtimeLoggerConfig {
    fn default() -> Self {
        Self {
            max_message_length: 128,
            num_slots: 32,
        }
    }
}

pub fn realtime_logger(config: RealtimeLoggerConfig) -> (RealtimeLogger, RealtimeLoggerMainThread) {
    #[cfg(debug_assertions)]
    let (mut debug_prod_1, debug_cons_1) = ringbuf::HeapRb::new(config.num_slots).split();
    #[cfg(debug_assertions)]
    let (debug_prod_2, debug_cons_2) = ringbuf::HeapRb::new(config.num_slots).split();

    let (mut error_prod_1, error_cons_1) = ringbuf::HeapRb::new(config.num_slots).split();
    let (error_prod_2, error_cons_2) = ringbuf::HeapRb::new(config.num_slots).split();

    #[cfg(debug_assertions)]
    for _ in 0..config.num_slots {
        let mut slot = String::new();
        slot.reserve_exact(config.max_message_length);

        debug_prod_1.try_push(slot).unwrap();
    }

    for _ in 0..config.num_slots {
        let mut slot = String::new();
        slot.reserve_exact(config.max_message_length);

        error_prod_1.try_push(slot).unwrap();
    }

    let shared_state = ArcGc::new(SharedState {
        message_too_long_occured: AtomicBool::new(false),
        not_enough_slots_occured: AtomicBool::new(false),
    });

    (
        RealtimeLogger {
            #[cfg(debug_assertions)]
            debug_prod: debug_prod_2,
            #[cfg(debug_assertions)]
            debug_cons: debug_cons_1,
            error_prod: error_prod_2,
            error_cons: error_cons_1,
            shared_state: ArcGc::clone(&shared_state),
            max_msg_length: config.max_message_length,
        },
        RealtimeLoggerMainThread {
            #[cfg(debug_assertions)]
            debug_prod: debug_prod_1,
            #[cfg(debug_assertions)]
            debug_cons: debug_cons_2,
            error_prod: error_prod_1,
            error_cons: error_cons_2,
            shared_state,
        },
    )
}

struct SharedState {
    message_too_long_occured: AtomicBool,
    not_enough_slots_occured: AtomicBool,
}

/// A helper used for realtime-safe logging on the audio thread.
pub struct RealtimeLogger {
    #[cfg(debug_assertions)]
    debug_prod: ringbuf::HeapProd<String>,
    #[cfg(debug_assertions)]
    debug_cons: ringbuf::HeapCons<String>,

    error_prod: ringbuf::HeapProd<String>,
    error_cons: ringbuf::HeapCons<String>,

    shared_state: ArcGc<SharedState>,

    max_msg_length: usize,
}

impl RealtimeLogger {
    /// The allocated capacity for each message slot.
    pub fn max_message_length(&self) -> usize {
        self.max_msg_length
    }

    /// Returns the number of slots that are available for debug messages.
    ///
    /// This will always return `0` when compiled without debug assertions.
    pub fn available_debug_slots(&self) -> usize {
        #[cfg(debug_assertions)]
        return self.debug_cons.occupied_len();
        #[cfg(not(debug_assertions))]
        return 0;
    }

    /// Returns the number of slots that are available for error messages.
    pub fn available_error_slots(&self) -> usize {
        self.error_cons.occupied_len()
    }

    /// Log the given debug message.
    ///
    /// *NOTE*, avoid using this method in the final release of your node.
    /// This is only meant for debugging purposes while developing.
    ///
    /// This will do nothing when compiled without debug assertions.
    #[allow(unused)]
    pub fn try_debug(&mut self, message: &str) -> Result<(), RealtimeLogError> {
        #[cfg(debug_assertions)]
        {
            if message.len() > self.max_msg_length {
                self.shared_state
                    .message_too_long_occured
                    .store(true, Ordering::Relaxed);
                return Err(RealtimeLogError::MessageTooLong);
            }

            let Some(mut slot) = self.debug_cons.try_pop() else {
                self.shared_state
                    .not_enough_slots_occured
                    .store(true, Ordering::Relaxed);
                return Err(RealtimeLogError::OutOfSlots);
            };

            slot.clear();
            slot.push_str(message);

            let _ = self.debug_prod.try_push(slot);
        }

        Ok(())
    }

    /// Log a debug message into the given string.
    ///
    /// This string is gauranteed to be empty and have an allocated capacity
    /// of at least [`RealtimeLogger::max_message_length`].
    ///
    /// *NOTE*, avoid using this method in the final release of your node.
    /// This is only meant for debugging purposes while developing.
    ///
    /// This will do nothing when compiled without debug assertions.
    #[allow(unused)]
    pub fn try_debug_with(&mut self, f: impl FnOnce(&mut String)) -> Result<(), RealtimeLogError> {
        #[cfg(debug_assertions)]
        {
            let Some(mut slot) = self.debug_cons.try_pop() else {
                self.shared_state
                    .not_enough_slots_occured
                    .store(true, Ordering::Relaxed);
                return Err(RealtimeLogError::OutOfSlots);
            };

            slot.clear();

            (f)(&mut slot);

            let _ = self.debug_prod.try_push(slot);
        }

        Ok(())
    }

    /// Log the given error message.
    pub fn try_error(&mut self, message: &str) -> Result<(), RealtimeLogError> {
        if message.len() > self.max_msg_length {
            self.shared_state
                .message_too_long_occured
                .store(true, Ordering::Relaxed);
            return Err(RealtimeLogError::MessageTooLong);
        }

        let Some(mut slot) = self.error_cons.try_pop() else {
            self.shared_state
                .not_enough_slots_occured
                .store(true, Ordering::Relaxed);
            return Err(RealtimeLogError::OutOfSlots);
        };

        slot.clear();
        slot.push_str(message);

        let _ = self.error_prod.try_push(slot);

        Ok(())
    }

    /// Log an error message into the given string.
    ///
    /// This string is gauranteed to be empty and have an allocated capacity
    /// of at least [`RealtimeLogger::max_message_length`].
    pub fn try_error_with(&mut self, f: impl FnOnce(&mut String)) -> Result<(), RealtimeLogError> {
        let Some(mut slot) = self.error_cons.try_pop() else {
            self.shared_state
                .not_enough_slots_occured
                .store(true, Ordering::Relaxed);
            return Err(RealtimeLogError::OutOfSlots);
        };

        slot.clear();

        (f)(&mut slot);

        let _ = self.error_prod.try_push(slot);

        Ok(())
    }
}

/// The main thread counterpart to a [`RealtimeLogger`].
pub struct RealtimeLoggerMainThread {
    #[cfg(debug_assertions)]
    debug_prod: ringbuf::HeapProd<String>,
    #[cfg(debug_assertions)]
    debug_cons: ringbuf::HeapCons<String>,

    error_prod: ringbuf::HeapProd<String>,
    error_cons: ringbuf::HeapCons<String>,

    shared_state: ArcGc<SharedState>,
}

impl RealtimeLoggerMainThread {
    /// Flush the queued log messages.
    pub fn flush(
        &mut self,
        mut log_error: impl FnMut(&str),
        #[cfg(debug_assertions)] mut log_debug: impl FnMut(&str),
    ) {
        if self
            .shared_state
            .message_too_long_occured
            .swap(false, Ordering::Relaxed)
        {
            (log_error)("One or more realtime log messages were dropped because they were too long. Please increase message capacity.");
        }
        if self
            .shared_state
            .not_enough_slots_occured
            .swap(false, Ordering::Relaxed)
        {
            (log_error)("One or more realtime log messages were dropped because the realtime logger ran out of slots. Please increase slot capacity.");
        }

        #[cfg(debug_assertions)]
        for slot in self.debug_cons.pop_iter() {
            (log_debug)(&slot);
            self.debug_prod.try_push(slot).unwrap();
        }

        for slot in self.error_cons.pop_iter() {
            (log_error)(&slot);
            self.error_prod.try_push(slot).unwrap();
        }
    }
}

#[derive(Debug, Clone, Copy, thiserror::Error)]
pub enum RealtimeLogError {
    /// There is not enough space to fit the message in the realtime log buffer.
    #[error("There is not enough space to fit the message in the realtime log buffer")]
    MessageTooLong,
    #[error("The realtime log buffer is out of slots")]
    OutOfSlots,
}
