use bevy_platform::time::Instant;

#[cfg(all(feature = "node_profiling", not(feature = "std")))]
use bevy_platform::prelude::Vec;

use crate::context::FirewheelBitFlags;
use crate::graph::CompiledSchedule;

#[cfg(feature = "node_profiling")]
use firewheel_core::node::NodeID;

pub(crate) fn profiler_channel(
    node_capacity: usize,
    #[cfg(feature = "node_profiling")] graph_out_node_id: NodeID,
) -> (ProfilerTx, ProfilerRx) {
    let (buffer_tx, buffer_rx) =
        triple_buffer::TripleBuffer::new(&ProfilingData::with_node_capacity(
            #[cfg(feature = "node_profiling")]
            node_capacity,
            #[cfg(feature = "node_profiling")]
            graph_out_node_id,
        ))
        .split();

    let now = Instant::now();

    #[allow(unused_mut)]
    let mut heap_data = ProfilerHeapData::new(node_capacity, false);
    // Initialize the list of nodes with the graph output node.
    #[cfg(feature = "node_profiling")]
    heap_data.nodes.push(NodeProfileData {
        node_id: graph_out_node_id,
        cpu_usage: 0.0,
    });

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
            heap_data,
            #[cfg(feature = "node_profiling")]
            is_profiling_nodes: false,
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

    heap_data: ProfilerHeapData,

    #[cfg(feature = "node_profiling")]
    is_profiling_nodes: bool,
    #[cfg(feature = "node_profiling")]
    node_profile_start_instant: Instant,
    #[cfg(feature = "node_profiling")]
    node_schedule_index: usize,
}

pub(crate) struct ProfilerHeapData {
    #[cfg(feature = "node_profiling")]
    nodes: Vec<NodeProfileData>,
    #[cfg(feature = "node_profiling")]
    node_cpu_sums: Vec<f64>,
    #[cfg(feature = "node_profiling")]
    triple_buf_allocations: [Vec<NodeProfileData>; 3],
}

impl ProfilerHeapData {
    pub fn new(node_capacity: usize, triple_buf_allocations: bool) -> Self {
        #[cfg(not(feature = "node_profiling"))]
        let _ = node_capacity;
        #[cfg(not(feature = "node_profiling"))]
        let _ = triple_buf_allocations;

        Self {
            #[cfg(feature = "node_profiling")]
            nodes: Vec::with_capacity(node_capacity),
            #[cfg(feature = "node_profiling")]
            node_cpu_sums: Vec::with_capacity(node_capacity),
            #[cfg(feature = "node_profiling")]
            triple_buf_allocations: if triple_buf_allocations {
                [
                    Vec::with_capacity(node_capacity),
                    Vec::with_capacity(node_capacity),
                    Vec::with_capacity(node_capacity),
                ]
            } else {
                [Vec::new(), Vec::new(), Vec::new()]
            },
        }
    }
}

impl ProfilerTx {
    pub fn new_schedule(
        &mut self,
        schedule: &CompiledSchedule,
        new_heap_data: &mut Option<ProfilerHeapData>,
    ) {
        #[cfg(not(feature = "node_profiling"))]
        let _ = schedule;

        if let Some(new_heap_data) = new_heap_data.as_mut() {
            core::mem::swap(&mut self.heap_data, new_heap_data);
        }

        #[cfg(feature = "node_profiling")]
        {
            self.heap_data.nodes.clear();
            self.heap_data.node_cpu_sums.clear();

            let graph_in_node_id = schedule.graph_in_node_id();

            self.heap_data.nodes.extend(
                schedule
                    .iter_node_ids()
                    // Don't count the graph input node since it is processed separately.
                    .filter(|node_id| *node_id != graph_in_node_id)
                    .map(|node_id| NodeProfileData {
                        node_id,
                        // TODO: Try to re-use old cpu usage data.
                        cpu_usage: 0.0,
                    }),
            );
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
            let new_is_profiling_nodes = flags.contains(FirewheelBitFlags::PROFILE_NODES);

            if new_is_profiling_nodes && !self.is_profiling_nodes {
                for node in self.heap_data.nodes.iter_mut() {
                    node.cpu_usage = 0.0;
                }
            }

            self.is_profiling_nodes = new_is_profiling_nodes;

            if self.is_profiling_nodes {
                self.heap_data.node_cpu_sums.clear();
                self.heap_data
                    .node_cpu_sums
                    .resize(self.heap_data.nodes.len(), 0.0);
            }
        }
    }

    pub fn begin_new_bookkeeping_part(&mut self) {
        if self.is_profiling_bookkeeping
            && let Some(now) = crate::time::now()
        {
            self.bookkeeping_start_instant = now;
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

        if self.is_profiling_nodes
            && let Some(now) = crate::time::now()
        {
            self.node_profile_start_instant = now;
        }
    }

    #[cfg(feature = "node_profiling")]
    pub fn node_completed(&mut self) {
        if self.is_profiling_nodes
            && let Some(new_profile_instant) = crate::time::now()
        {
            let node_cpu_usage = new_profile_instant
                .duration_since(self.node_profile_start_instant)
                .as_secs_f64()
                * self.total_cpu_seconds_recip;
            self.heap_data.node_cpu_sums[self.node_schedule_index] += node_cpu_usage;

            self.node_profile_start_instant = new_profile_instant;
            self.node_schedule_index += 1;
        }
    }

    pub fn process_loop_completed(&mut self) {
        let Some(now) = crate::time::now() else {
            return;
        };

        #[cfg(feature = "node_profiling")]
        if self.is_profiling_nodes {
            for (node, &sum) in self
                .heap_data
                .nodes
                .iter_mut()
                .zip(self.heap_data.node_cpu_sums.iter())
            {
                node.cpu_usage = node.cpu_usage.max(sum);
            }
        }

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
                    .then_some(self.bookkeeping_cpu_usage);

                #[cfg(feature = "node_profiling")]
                if self.is_profiling_nodes {
                    if data.nodes.capacity() < self.heap_data.nodes.len()
                        && let Some(new_vec) = self
                            .heap_data
                            .triple_buf_allocations
                            .iter_mut()
                            .find(|v| v.capacity() >= self.heap_data.nodes.len())
                    {
                        core::mem::swap(&mut data.nodes, new_vec);
                    }

                    data.nodes.clear();
                    data.nodes.extend_from_slice(&self.heap_data.nodes);
                } else {
                    data.nodes.clear();
                }
            }

            self.buffer_tx.publish();
            self.version_counter += 1;

            self.overall_cpu_usage = 0.0;
            self.bookkeeping_cpu_usage = 0.0;

            #[cfg(feature = "node_profiling")]
            if self.is_profiling_nodes {
                for node in self.heap_data.nodes.iter_mut() {
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
    /// underrun has occurred.
    ///
    /// The value is the maximum value that has occurred since the last time
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
    /// The value is the maximum value that has occurred since the last time
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
    fn with_node_capacity(
        #[cfg(feature = "node_profiling")] node_capacity: usize,
        #[cfg(feature = "node_profiling")] graph_out_id: NodeID,
    ) -> Self {
        #[cfg(feature = "node_profiling")]
        let mut nodes = Vec::with_capacity(node_capacity);
        #[cfg(feature = "node_profiling")]
        nodes.push(NodeProfileData {
            node_id: graph_out_id,
            cpu_usage: 0.0,
        });

        Self {
            version: 0,
            overall_cpu_usage: 0.0,
            engine_bookkeeping_cpu_usage: None,
            #[cfg(feature = "node_profiling")]
            nodes,
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
    /// The value is the maximum value that has occurred since the last time
    /// the profiling information was fetched with
    /// [`FirewheelContext::profile_info()`](crate::context::FirewheelContext::profile_info).
    pub cpu_usage: f64,
}
