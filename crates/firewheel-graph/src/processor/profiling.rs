use firewheel_core::log::RealtimeLogger;

use bevy_platform::time::Instant;

use crate::context::FirewheelBitFlags;
use crate::graph::CompiledSchedule;

#[cfg(feature = "node_profiling")]
use crate::processor::BufferOutOfSpaceMode;
#[cfg(feature = "node_profiling")]
use firewheel_core::node::NodeID;

pub(crate) fn profiler_channel(
    #[cfg(feature = "node_profiling")] node_capacity: usize,
    #[cfg(feature = "node_profiling")] buffer_out_of_space_mode: BufferOutOfSpaceMode,
) -> (ProfilerTx, ProfilerRx) {
    let (buffer_tx, buffer_rx) =
        triple_buffer::TripleBuffer::new(&ProfilingData::with_node_capacity(
            #[cfg(feature = "node_profiling")]
            node_capacity,
        ))
        .split();

    let now = Instant::now();

    (
        ProfilerTx {
            buffer_tx,
            version_counter: 0,
            total_cpu_seconds_recip: 0.0,
            proc_start_instant: now,
            overall_cpu_usage: 0.0,
            bookkeeping_cpu_usage: 0.0,
            bookkeeping_cpu_usage_sum: 0.0,
            is_profiling_bookkeeping: false,
            bookkeeping_start_instant: now,
            #[cfg(feature = "node_profiling")]
            nodes: Vec::with_capacity(node_capacity),
            #[cfg(feature = "node_profiling")]
            node_cpu_sums: Vec::with_capacity(node_capacity),
            #[cfg(feature = "node_profiling")]
            node_capacity,
            #[cfg(feature = "node_profiling")]
            has_enough_node_capacity: true,
            #[cfg(feature = "node_profiling")]
            is_profiling_nodes: false,
            #[cfg(feature = "node_profiling")]
            buffer_out_of_space_mode,
            #[cfg(feature = "node_profiling")]
            node_profile_start_instant: now,
            #[cfg(feature = "node_profiling")]
            node_schedule_index: 0,
        },
        ProfilerRx { buffer_rx },
    )
}

pub(crate) struct ProfilerTx {
    buffer_tx: triple_buffer::Input<ProfilingData>,
    version_counter: u64,
    total_cpu_seconds_recip: f64,
    proc_start_instant: Instant,

    overall_cpu_usage: f64,
    bookkeeping_cpu_usage: f64,
    bookkeeping_cpu_usage_sum: f64,
    is_profiling_bookkeeping: bool,
    bookkeeping_start_instant: Instant,

    #[cfg(feature = "node_profiling")]
    nodes: Vec<NodeProfileData>,
    #[cfg(feature = "node_profiling")]
    node_cpu_sums: Vec<f64>,
    #[cfg(feature = "node_profiling")]
    node_capacity: usize,
    #[cfg(feature = "node_profiling")]
    has_enough_node_capacity: bool,
    #[cfg(feature = "node_profiling")]
    is_profiling_nodes: bool,
    #[cfg(feature = "node_profiling")]
    buffer_out_of_space_mode: BufferOutOfSpaceMode,
    #[cfg(feature = "node_profiling")]
    node_profile_start_instant: Instant,
    #[cfg(feature = "node_profiling")]
    node_schedule_index: usize,
}

impl ProfilerTx {
    pub fn new_schedule(&mut self, schedule: &CompiledSchedule, logger: &mut RealtimeLogger) {
        #[cfg(not(feature = "node_profiling"))]
        {
            let _ = schedule;
            let _ = logger;
        }

        #[cfg(feature = "node_profiling")]
        {
            let num_nodes = schedule.num_nodes();

            // TODO: Try to re-use old data.
            self.nodes.clear();
            self.node_cpu_sums.clear();

            if self.node_capacity < num_nodes {
                match self.buffer_out_of_space_mode {
                    BufferOutOfSpaceMode::AllocateOnAudioThread => {
                        let _ = logger.try_error("Firewheel node profiling buffer is full! Please increase FirewheelConfig::initial_node_capacity to avoid audio glitches.");

                        self.node_capacity = (num_nodes * 2).next_power_of_two();
                        self.nodes.reserve(self.node_capacity);
                        self.node_cpu_sums.reserve(self.node_capacity);
                        self.has_enough_node_capacity = true;
                    }
                    BufferOutOfSpaceMode::Panic => {
                        panic!("Firewheel node profiling buffer is full! Please increase FirewheelConfig::initial_node_capacity.");
                    }
                    BufferOutOfSpaceMode::DropEvents => {
                        let _ = logger.try_error("Firewheel node profiling buffer is full! Please increase FirewheelConfig::initial_node_capacity.");
                        self.has_enough_node_capacity = false;
                    }
                }
            } else {
                self.has_enough_node_capacity = true;
            }

            if self.has_enough_node_capacity {
                let graph_in_node_id = schedule.graph_in_node_id();

                self.nodes.extend(
                    schedule
                        .iter_node_ids()
                        // Don't count the graph input node since it is processed separately.
                        .filter(|node_id| *node_id != graph_in_node_id)
                        .map(|node_id| NodeProfileData {
                            node_id,
                            cpu_usage: 0.0,
                        }),
                );
            }
        }
    }

    pub fn new_process_loop(
        &mut self,
        proc_start_instant: Instant,
        total_cpu_seconds_recip: f64,
        flags: &FirewheelBitFlags,
    ) {
        self.proc_start_instant = proc_start_instant;
        self.total_cpu_seconds_recip = total_cpu_seconds_recip;

        self.bookkeeping_cpu_usage_sum = 0.0;
        self.bookkeeping_start_instant = proc_start_instant;

        let new_is_profiling_bookkeeping =
            flags.contains(FirewheelBitFlags::PROFILE_ENGINE_BOOKKEEPING);
        if new_is_profiling_bookkeeping && !self.is_profiling_bookkeeping {
            self.bookkeeping_cpu_usage = 0.0;
        }
        self.is_profiling_bookkeeping = new_is_profiling_bookkeeping;

        #[cfg(feature = "node_profiling")]
        {
            let new_is_profiling_nodes =
                flags.contains(FirewheelBitFlags::PROFILE_NODES) && self.has_enough_node_capacity;

            if new_is_profiling_nodes && !self.is_profiling_nodes {
                for node in self.nodes.iter_mut() {
                    node.cpu_usage = 0.0;
                }
            }

            self.is_profiling_nodes = new_is_profiling_nodes;

            if self.is_profiling_nodes {
                self.node_cpu_sums.clear();
                self.node_cpu_sums.resize(self.nodes.len(), 0.0);
            }
        }
    }

    pub fn begin_new_bookkeeping_part(&mut self) {
        if self.is_profiling_bookkeeping {
            self.bookkeeping_start_instant = Instant::now();
        }
    }

    pub fn bookkeeping_part_completed(&mut self) {
        if self.is_profiling_bookkeeping {
            let cpu_usage = self.bookkeeping_start_instant.elapsed().as_secs_f64()
                * self.total_cpu_seconds_recip;
            self.bookkeeping_cpu_usage_sum += cpu_usage;
        }
    }

    #[cfg(feature = "node_profiling")]
    pub fn begin_node_profiling(&mut self) {
        self.node_schedule_index = 0;

        if self.is_profiling_nodes {
            self.node_profile_start_instant = Instant::now();
        }
    }

    #[cfg(feature = "node_profiling")]
    pub fn node_completed(&mut self) {
        if self.is_profiling_nodes {
            let new_profile_instant = Instant::now();

            let node_cpu_usage = new_profile_instant
                .duration_since(self.node_profile_start_instant)
                .as_secs_f64()
                * self.total_cpu_seconds_recip;
            self.node_cpu_sums[self.node_schedule_index] += node_cpu_usage;

            self.node_profile_start_instant = new_profile_instant;
            self.node_schedule_index += 1;
        }
    }

    pub fn process_loop_completed(&mut self) {
        #[cfg(feature = "node_profiling")]
        if self.is_profiling_nodes {
            for (node, &sum) in self.nodes.iter_mut().zip(self.node_cpu_sums.iter()) {
                node.cpu_usage = node.cpu_usage.max(sum);
            }
        }

        let now = Instant::now();

        let overall_cpu_usage = now.duration_since(self.proc_start_instant).as_secs_f64()
            * self.total_cpu_seconds_recip;
        self.overall_cpu_usage = self.overall_cpu_usage.max(overall_cpu_usage);

        if self.is_profiling_bookkeeping {
            let cpu_usage = now
                .duration_since(self.bookkeeping_start_instant)
                .as_secs_f64()
                * self.total_cpu_seconds_recip;
            self.bookkeeping_cpu_usage_sum += cpu_usage;

            self.bookkeeping_cpu_usage = self
                .bookkeeping_cpu_usage
                .max(self.bookkeeping_cpu_usage_sum);
        }

        if self.buffer_tx.consumed() || self.version_counter == 0 {
            {
                let data = self.buffer_tx.input_buffer_mut();

                data.version = self.version_counter;
                data.overall_cpu_usage = self.overall_cpu_usage;

                data.engine_bookkeeping_cpu_usage = self
                    .is_profiling_bookkeeping
                    .then(|| self.bookkeeping_cpu_usage);

                #[cfg(feature = "node_profiling")]
                data.nodes.clear();

                #[cfg(feature = "node_profiling")]
                if self.is_profiling_nodes {
                    data.nodes.reserve(self.node_capacity);
                    data.nodes.extend_from_slice(&self.nodes);
                }
            }

            self.buffer_tx.publish();
            self.version_counter += 1;

            self.overall_cpu_usage = 0.0;
            self.bookkeeping_cpu_usage = 0.0;

            #[cfg(feature = "node_profiling")]
            if self.is_profiling_nodes {
                for node in self.nodes.iter_mut() {
                    node.cpu_usage = 0.0;
                }
            }
        }
    }
}

pub(crate) struct ProfilerRx {
    buffer_rx: triple_buffer::Output<ProfilingData>,
}

impl ProfilerRx {
    pub fn fetch_info(&mut self) -> &ProfilingData {
        self.buffer_rx.read()
    }
}

/// Performance profiling information of a Firewheel Processor.
#[derive(Default, Debug, Clone)]
pub struct ProfilingData {
    /// The number of times the profiling data has been updated.
    pub version: u64,

    /// The overall CPU usage of the entire Firewheel Processor's process method.
    ///
    /// A value of `0.0` means `0%` CPU usage, a value of `1.0` means `100%`
    /// CPU usage of the total alloted time, and values above `1.0` means an
    /// underrun has occured.
    ///
    /// The value is the maximum value that has occured since the last time
    /// the profiling information was fetched with
    /// [`FirewheelContext::profiling_data()`](crate::context::FirewheelContext::profiling_data).
    pub overall_cpu_usage: f64,

    /// The CPU usage of engine bookkeeping operations such as message handling,
    /// event sorting, event searching, and final output processing.
    ///
    /// A value of `0.0` means `0%` CPU usage, and a value of `1.0` means `100%`
    /// CPU usage of the total allotted time. (If instead you want the fraction
    /// of time spent relative to the total time spent in the Firewheel process
    /// method, it can be found with `bookkeeping_cpu_usage / overall_cpu_usage`.)
    ///
    /// The value is the maximum value that has occured since the last time
    /// the profiling information was fetched with
    /// [`FirewheelContext::profiling_data()`](crate::context::FirewheelContext::profiling_data).
    ///
    /// If [`FirewheelFlags::profile_engine_bookkeeping`](crate::context::FirewheelFlags::profile_engine_bookkeeping)
    /// is set to `false` (which it is by default), then this will be `None`.
    pub engine_bookkeeping_cpu_usage: Option<f64>,

    /// The profiling information of individual nodes.
    ///
    /// The order in which nodes appear is not defined.
    ///
    /// This may be empty if [`FirewheelFlags::profile_nodes`](crate::context::FirewheelFlags::profile_nodes)
    /// is set to `false` (which it is by default) or if there was an error
    /// with running out of buffer space.
    #[cfg(feature = "node_profiling")]
    pub nodes: Vec<NodeProfileData>,
}

impl ProfilingData {
    fn with_node_capacity(#[cfg(feature = "node_profiling")] node_capacity: usize) -> Self {
        Self {
            version: 0,
            overall_cpu_usage: 0.0,
            engine_bookkeeping_cpu_usage: None,
            #[cfg(feature = "node_profiling")]
            nodes: vec![NodeProfileData::default(); node_capacity],
        }
    }
}

/// Performance profiling information of a Firewheel audio node.
#[cfg(feature = "node_profiling")]
#[derive(Default, Debug, Clone, Copy, PartialEq)]
pub struct NodeProfileData {
    /// The ID of the node.
    pub node_id: NodeID,

    /// The CPU usage of this node.
    ///
    /// A value of `0.0` means `0%` CPU usage, and a value of `1.0` means `100%`
    /// CPU usage of the total allotted time. (If instead you want the fraction
    /// of time spent relative to the total time spent in the Firewheel process
    /// method, it can be found with `cpu_usage / ProfileData::overall_cpu_usage`.)
    ///
    /// The value is the maximum value that has occured since the last time
    /// the profiling information was fetched with
    /// [`FirewheelContext::profile_info()`](crate::context::FirewheelContext::profile_info).
    pub cpu_usage: f64,
}
