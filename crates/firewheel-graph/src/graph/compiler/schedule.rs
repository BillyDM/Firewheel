use arrayvec::ArrayVec;
use core::fmt::Debug;
use smallvec::SmallVec;
use thunderdome::Arena;

use firewheel_core::{
    channel_config::MAX_CHANNELS,
    mask::{ConnectedMask, ConstantMask, MaskType, SilenceMask},
    node::{AudioNodeProcessor, ProcBuffers, ProcessStatus},
};

use crate::context::FirewheelFlags;

use super::{InsertedSum, NodeID};

#[cfg(not(feature = "std"))]
use bevy_platform::prelude::{vec, Box, Vec};

/// A special scheduled node that has zero inputs and outputs. It
/// processes before all other nodes in the graph.
#[derive(Clone)]
pub(super) struct PreProcNode {
    /// The node ID
    pub id: NodeID,
    pub debug_name: &'static str,
}

impl Debug for PreProcNode {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "{{ {}-{}-{}",
            self.debug_name,
            self.id.0.slot(),
            self.id.0.generation()
        )
    }
}

/// A [ScheduledNode] is a node that has been assigned buffers
/// and a place in the schedule.
#[derive(Clone)]
pub(super) struct ScheduledNode {
    /// The node ID
    pub id: NodeID,
    pub debug_name: &'static str,

    /// The assigned input buffers.
    pub input_buffers: SmallVec<[InBufferAssignment; 4]>,
    /// The assigned output buffers.
    pub output_buffers: SmallVec<[OutBufferAssignment; 4]>,

    pub in_connected_mask: ConnectedMask,
    pub out_connected_mask: ConnectedMask,

    pub sum_inputs: Vec<InsertedSum>,
}

impl ScheduledNode {
    pub fn new(id: NodeID, debug_name: &'static str) -> Self {
        Self {
            id,
            debug_name,
            input_buffers: SmallVec::new(),
            output_buffers: SmallVec::new(),
            in_connected_mask: ConnectedMask::default(),
            out_connected_mask: ConnectedMask::default(),
            sum_inputs: Vec::new(),
        }
    }
}

impl Debug for ScheduledNode {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "{{ {}-{}-{}",
            self.debug_name,
            self.id.0.slot(),
            self.id.0.generation()
        )?;

        if !self.sum_inputs.is_empty() {
            write!(f, " | sums: [")?;

            for (i, sum_input) in self.sum_inputs.iter().enumerate() {
                write!(f, "{{ in: [")?;
                write!(f, "{}", sum_input.input_buffers[0].buffer_index)?;
                for in_buf in sum_input.input_buffers.iter().skip(1) {
                    write!(f, ", {}", in_buf.buffer_index)?;
                }

                write!(f, "], out: {} }}", sum_input.output_buffer.buffer_index)?;

                if i != self.sum_inputs.len() - 1 && self.sum_inputs.len() > 1 {
                    write!(f, ", ")?;
                }
            }

            write!(f, "]")?;
        }

        if !self.input_buffers.is_empty() {
            write!(f, " | in: [")?;

            write!(f, "{}", self.input_buffers[0].buffer_index)?;
            for b in self.input_buffers.iter().skip(1) {
                write!(f, ", {}", b.buffer_index)?;
            }

            write!(f, "]")?;
        }

        if !self.output_buffers.is_empty() {
            write!(f, " | out: [")?;

            write!(f, "{}", self.output_buffers[0].buffer_index)?;
            for b in self.output_buffers.iter().skip(1) {
                write!(f, ", {}", b.buffer_index)?;
            }

            write!(f, "]")?;
        }

        if !self.input_buffers.is_empty() {
            write!(f, " | in_clear: [")?;

            write!(
                f,
                "{}",
                if self.input_buffers[0].should_clear {
                    'y'
                } else {
                    'n'
                }
            )?;
            for b in self.input_buffers.iter().skip(1) {
                write!(f, ", {}", if b.should_clear { 'y' } else { 'n' })?;
            }

            write!(f, "]")?;
        }

        write!(f, " }}")
    }
}

/// Represents a single buffer assigned to an input port
#[derive(Copy, Clone, Debug)]
pub(super) struct InBufferAssignment {
    /// The index of the buffer assigned
    pub buffer_index: usize,
    /// Whether the engine should clear the buffer before
    /// passing it to a process
    pub should_clear: bool,
}

/// Represents a single buffer assigned to an output port
#[derive(Copy, Clone, Debug)]
pub(super) struct OutBufferAssignment {
    /// The index of the buffer assigned
    pub buffer_index: usize,
}

pub struct NodeHeapData {
    pub id: NodeID,
    pub processor: Box<dyn AudioNodeProcessor>,
    pub is_pre_process: bool,
    //pub event_buffer_indices: Vec<u32>,
}

pub struct ScheduleHeapData {
    pub schedule: CompiledSchedule,
    pub nodes_to_remove: Vec<NodeID>,
    pub removed_nodes: Vec<NodeHeapData>,
    pub new_node_processors: Vec<NodeHeapData>,
    pub new_node_arena: Option<Arena<crate::processor::NodeEntry>>,
}

impl ScheduleHeapData {
    pub fn new(
        schedule: CompiledSchedule,
        nodes_to_remove: Vec<NodeID>,
        new_node_processors: Vec<NodeHeapData>,
        new_node_arena: Option<Arena<crate::processor::NodeEntry>>,
    ) -> Self {
        let num_nodes_to_remove = nodes_to_remove.len();

        Self {
            schedule,
            nodes_to_remove,
            removed_nodes: Vec::with_capacity(num_nodes_to_remove),
            new_node_processors,
            new_node_arena,
        }
    }
}

impl Debug for ScheduleHeapData {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let new_node_processors: Vec<NodeID> =
            self.new_node_processors.iter().map(|n| n.id).collect();

        f.debug_struct("ScheduleHeapData")
            .field("schedule", &self.schedule)
            .field("nodes_to_remove", &self.nodes_to_remove)
            .field("new_node_processors", &new_node_processors)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BufferFlags {
    silent: bool,
    constant: bool,
    frames: u16,
}

impl BufferFlags {
    fn set_silent(&mut self, silent: bool, frames: u16) {
        self.silent = silent;
        self.constant = silent;
        self.frames = frames;
    }
}

pub(crate) struct ScheduleProcStatus {
    pub status: ProcessStatus,
    pub is_bypass_declicking: bool,
    pub is_bypassed: bool,
}

/// A [CompiledSchedule] is the output of the graph compiler.
pub struct CompiledSchedule {
    pre_proc_nodes: Vec<PreProcNode>,
    schedule: Vec<ScheduledNode>,

    buffers: Vec<f32>,
    buffer_flags: Vec<BufferFlags>,
    num_buffers: usize,

    /// Containts two buffers, one for wet and one for dry.
    bypass_gain_buffer: Vec<f32>,

    max_block_frames: usize,
    graph_in_node_id: NodeID,
}

impl Debug for CompiledSchedule {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        writeln!(f, "CompiledSchedule {{")?;

        if !self.pre_proc_nodes.is_empty() {
            writeln!(f, "    pre process nodes: {{")?;

            for n in self.pre_proc_nodes.iter() {
                writeln!(f, "        {:?}", n)?;
            }

            writeln!(f, "    }}")?;
        }

        writeln!(f, "    schedule: {{")?;

        for n in self.schedule.iter() {
            writeln!(f, "        {:?}", n)?;
        }

        writeln!(f, "    }}")?;

        writeln!(f, "    num_buffers: {}", self.num_buffers)?;
        writeln!(f, "    max_block_frames: {}", self.max_block_frames)?;

        writeln!(f, "}}")
    }
}

impl CompiledSchedule {
    pub(super) fn new(
        pre_proc_nodes: Vec<PreProcNode>,
        schedule: Vec<ScheduledNode>,
        num_buffers: usize,
        max_block_frames: usize,
        graph_in_node_id: NodeID,
    ) -> Self {
        assert!(max_block_frames <= u16::MAX as usize);

        let mut buffers = Vec::new();
        buffers.reserve_exact(num_buffers * max_block_frames);
        buffers.resize(num_buffers * max_block_frames, 0.0);

        let mut bypass_gain_buffer = Vec::new();
        // Two buffers, one for wet and one for dry.
        bypass_gain_buffer.reserve_exact(max_block_frames * 2);
        bypass_gain_buffer.resize(max_block_frames * 2, 0.0);

        Self {
            pre_proc_nodes,
            schedule,
            buffers,
            buffer_flags: vec![
                BufferFlags {
                    silent: false,
                    constant: false,
                    frames: 0,
                };
                num_buffers
            ],
            num_buffers,
            bypass_gain_buffer,
            max_block_frames,
            graph_in_node_id,
        }
    }

    pub fn max_block_frames(&self) -> usize {
        self.max_block_frames
    }

    pub fn prepare_graph_inputs(
        &mut self,
        frames: usize,
        num_stream_inputs: usize,
        fill_inputs: impl FnOnce(&mut [&mut [f32]]) -> SilenceMask,
    ) {
        let frames = frames.min(self.max_block_frames);
        let frames_u16 = frames as u16;
        let buffers_ptr = self.buffers.as_mut_ptr();
        let max_block_frames = self.max_block_frames;

        let graph_in_node = self.schedule.first().unwrap();

        let mut inputs: ArrayVec<&mut [f32], MAX_CHANNELS> = ArrayVec::new();

        let fill_input_len = num_stream_inputs.min(graph_in_node.output_buffers.len());

        for i in 0..fill_input_len {
            // SAFETY: Output buffer indexes within a single node are guaranteed non-overlapping by the buffer allocator.
            inputs.push(unsafe {
                core::slice::from_raw_parts_mut(
                    buffers_ptr
                        .add(graph_in_node.output_buffers[i].buffer_index * max_block_frames),
                    frames,
                )
            });
        }

        let silence_mask = (fill_inputs)(inputs.as_mut_slice());

        for i in 0..fill_input_len {
            let buffer_index = graph_in_node.output_buffers[i].buffer_index;
            flag_mut(&mut self.buffer_flags, buffer_index)
                .set_silent(silence_mask.is_channel_silent(i), frames_u16);
        }

        if fill_input_len < graph_in_node.output_buffers.len() {
            for b in graph_in_node.output_buffers.iter().skip(fill_input_len) {
                // SAFETY: Each buffer index is used once per iteration.
                let buf_slice = unsafe {
                    core::slice::from_raw_parts_mut(
                        buffers_ptr.add(b.buffer_index * max_block_frames),
                        frames,
                    )
                };
                buf_slice.fill(0.0);

                flag_mut(&mut self.buffer_flags, b.buffer_index).set_silent(true, frames_u16);
            }
        }

        // Make sure all buffers that are marked as silent/constant remain that
        // way if the number of frames have changed.
        for i in 0..self.num_buffers {
            let flag = flag_mut(&mut self.buffer_flags, i);

            if (flag.silent || flag.constant) && flag.frames < frames_u16 {
                // SAFETY: Each buffer index `i` is used once per iteration.
                let buf_slice = unsafe {
                    core::slice::from_raw_parts_mut(buffers_ptr.add(i * max_block_frames), frames)
                };

                if flag.silent {
                    buf_slice[flag.frames as usize..frames].fill(0.0);
                } else {
                    let val = buf_slice[0];
                    buf_slice[flag.frames as usize..frames].fill(val);
                }

                flag.frames = frames_u16;
            }
        }
    }

    pub fn read_graph_outputs(
        &mut self,
        frames: usize,
        num_stream_outputs: usize,
        read_outputs: impl FnOnce(&[&[f32]], SilenceMask),
    ) {
        let frames = frames.min(self.max_block_frames);
        let buffers_ptr = self.buffers.as_mut_ptr();
        let max_block_frames = self.max_block_frames;

        let graph_out_node = self.schedule.last().unwrap();

        let mut outputs: ArrayVec<&[f32], MAX_CHANNELS> = ArrayVec::new();

        let mut silence_mask = SilenceMask::NONE_SILENT;

        let read_output_len = num_stream_outputs.min(graph_out_node.input_buffers.len());

        for i in 0..read_output_len {
            let buffer_index = graph_out_node.input_buffers[i].buffer_index;

            if flag_mut(&mut self.buffer_flags, buffer_index).silent {
                silence_mask.set_channel(i, true);
            }

            // SAFETY: Input buffer indexes within a single node are guaranteed non-overlapping by the buffer allocator.
            outputs.push(unsafe {
                core::slice::from_raw_parts_mut(
                    buffers_ptr.add(buffer_index * max_block_frames),
                    frames,
                )
            });
        }

        (read_outputs)(outputs.as_slice(), silence_mask);
    }

    #[cfg(feature = "scheduled_events")]
    pub fn has_pre_proc_nodes(&self) -> bool {
        !self.pre_proc_nodes.is_empty()
    }

    pub fn process(
        &mut self,
        frames: usize,
        flags: FirewheelFlags,
        mut process: impl FnMut(
            NodeID,
            SilenceMask,
            SilenceMask,
            ConstantMask,
            ConstantMask,
            ConnectedMask,
            ConnectedMask,
            ProcBuffers,
            &mut [f32],
        ) -> ScheduleProcStatus,
    ) {
        let frames = frames.min(self.max_block_frames);
        let frames_u16 = frames as u16;
        let buffers_ptr = self.buffers.as_mut_ptr();
        let max_block_frames = self.max_block_frames;
        let debug_force_clear_buffers = flags.force_clear_buffers;

        let mut inputs: ArrayVec<&[f32], MAX_CHANNELS> = ArrayVec::new();
        let mut outputs: ArrayVec<&mut [f32], MAX_CHANNELS> = ArrayVec::new();

        for pre_proc_node in self.pre_proc_nodes.iter() {
            if pre_proc_node.id == self.graph_in_node_id {
                continue;
            }

            (process)(
                pre_proc_node.id,
                SilenceMask::NONE_SILENT,
                SilenceMask::NONE_SILENT,
                ConstantMask::NONE_CONSTANT,
                ConstantMask::NONE_CONSTANT,
                ConnectedMask::NONE_CONNECTED,
                ConnectedMask::NONE_CONNECTED,
                ProcBuffers {
                    inputs: &[],
                    outputs: &mut [],
                },
                &mut self.bypass_gain_buffer,
            );
        }

        for scheduled_node in self.schedule.iter() {
            if scheduled_node.id == self.graph_in_node_id {
                continue;
            }

            for inserted_sum in scheduled_node.sum_inputs.iter() {
                // SAFETY: buffers_ptr is derived from &mut self.buffers.
                // Buffer indexes in sum_inputs are guaranteed non-overlapping by the buffer allocator.
                unsafe {
                    sum_inputs(
                        inserted_sum,
                        buffers_ptr,
                        &mut self.buffer_flags,
                        max_block_frames,
                        frames,
                    );
                }
            }

            let mut in_silence_mask = SilenceMask::NONE_SILENT;
            let mut out_silence_mask = SilenceMask::NONE_SILENT;
            let mut in_constant_mask = ConstantMask::NONE_CONSTANT;
            let mut out_constant_mask = ConstantMask::NONE_CONSTANT;

            inputs.clear();
            outputs.clear();

            for (i, b) in scheduled_node.input_buffers.iter().enumerate() {
                // SAFETY: Input buffer indexes within a single node are guaranteed non-overlapping by the buffer allocator.
                let buf = unsafe {
                    core::slice::from_raw_parts_mut(
                        buffers_ptr.add(b.buffer_index * max_block_frames),
                        frames,
                    )
                };
                let flag = flag_mut(&mut self.buffer_flags, b.buffer_index);

                if b.should_clear && (!flag.silent || debug_force_clear_buffers) {
                    buf.fill(0.0);
                    flag.set_silent(true, frames_u16);
                }

                in_silence_mask.set_channel(i, flag.silent);
                in_constant_mask.set_channel(i, flag.constant);

                inputs.push(buf);
            }

            for (i, b) in scheduled_node.output_buffers.iter().enumerate() {
                // SAFETY: Output buffer indexes within a single node are guaranteed non-overlapping by the buffer allocator.
                let buf = unsafe {
                    core::slice::from_raw_parts_mut(
                        buffers_ptr.add(b.buffer_index * max_block_frames),
                        frames,
                    )
                };
                let flag = flag_mut(&mut self.buffer_flags, b.buffer_index);

                if debug_force_clear_buffers {
                    buf.fill(0.0);
                    flag.set_silent(true, frames_u16);
                }

                out_silence_mask.set_channel(i, flag.silent);
                out_constant_mask.set_channel(i, flag.constant);

                outputs.push(buf);
            }

            let status = (process)(
                scheduled_node.id,
                in_silence_mask,
                out_silence_mask,
                in_constant_mask,
                out_constant_mask,
                scheduled_node.in_connected_mask,
                scheduled_node.out_connected_mask,
                ProcBuffers {
                    inputs: inputs.as_slice(),
                    outputs: outputs.as_mut_slice(),
                },
                &mut self.bypass_gain_buffer,
            );

            let mut bypass = || {
                for (in_buf, out_buf) in scheduled_node
                    .input_buffers
                    .iter()
                    .zip(scheduled_node.output_buffers.iter())
                {
                    let in_flag = *flag_mut(&mut self.buffer_flags, in_buf.buffer_index);
                    let out_flag = flag_mut(&mut self.buffer_flags, out_buf.buffer_index);

                    if in_flag.silent {
                        if !out_flag.silent || debug_force_clear_buffers {
                            // SAFETY: Each buffer index is used once per iteration.
                            unsafe {
                                core::slice::from_raw_parts_mut(
                                    buffers_ptr.add(out_buf.buffer_index * max_block_frames),
                                    frames,
                                )
                            }
                            .fill(0.0);
                            out_flag.set_silent(true, frames_u16);
                        }
                    } else {
                        // SAFETY: Input and output buffer indexes are guaranteed distinct by the buffer allocator.
                        let in_buf_slice = unsafe {
                            core::slice::from_raw_parts_mut(
                                buffers_ptr.add(in_buf.buffer_index * max_block_frames),
                                frames,
                            )
                        };
                        let out_buf_slice = unsafe {
                            core::slice::from_raw_parts_mut(
                                buffers_ptr.add(out_buf.buffer_index * max_block_frames),
                                frames,
                            )
                        };

                        out_buf_slice.copy_from_slice(in_buf_slice);
                        *out_flag = in_flag;
                    }
                }

                for b in scheduled_node
                    .output_buffers
                    .iter()
                    .skip(scheduled_node.input_buffers.len())
                {
                    let s = flag_mut(&mut self.buffer_flags, b.buffer_index);

                    if !s.silent || debug_force_clear_buffers {
                        // SAFETY: Each buffer index is used once per iteration.
                        unsafe {
                            core::slice::from_raw_parts_mut(
                                buffers_ptr.add(b.buffer_index * max_block_frames),
                                frames,
                            )
                        }
                        .fill(0.0);
                        s.set_silent(true, frames_u16);
                    }
                }
            };

            if status.is_bypassed {
                bypass();
            } else {
                match status.status {
                    ProcessStatus::ClearAllOutputs => {
                        // Clear output buffers which need cleared.
                        for b in scheduled_node.output_buffers.iter() {
                            let flag = flag_mut(&mut self.buffer_flags, b.buffer_index);

                            if !flag.silent || debug_force_clear_buffers {
                                // SAFETY: Each buffer index is used once per iteration.
                                unsafe {
                                    core::slice::from_raw_parts_mut(
                                        buffers_ptr.add(b.buffer_index * max_block_frames),
                                        frames,
                                    )
                                }
                                .fill(0.0);
                                flag.set_silent(true, frames_u16);
                            }
                        }
                    }
                    ProcessStatus::Bypass => bypass(),
                    ProcessStatus::OutputsModified => {
                        for b in scheduled_node.output_buffers.iter() {
                            flag_mut(&mut self.buffer_flags, b.buffer_index)
                                .set_silent(false, frames_u16);
                        }
                    }
                    ProcessStatus::OutputsModifiedWithMask(out_mask) => match out_mask {
                        MaskType::Silence(silence_mask) => {
                            for (i, b) in scheduled_node.output_buffers.iter().enumerate() {
                                flag_mut(&mut self.buffer_flags, b.buffer_index)
                                    .set_silent(silence_mask.is_channel_silent(i), frames_u16);
                            }
                        }
                        MaskType::Constant(constant_mask) => {
                            for (i, b) in scheduled_node.output_buffers.iter().enumerate() {
                                let flag = flag_mut(&mut self.buffer_flags, b.buffer_index);

                                if constant_mask.is_channel_constant(i) {
                                    flag.constant = true;
                                    // SAFETY: Each buffer index is used once per iteration.
                                    flag.silent = unsafe {
                                        core::slice::from_raw_parts_mut(
                                            buffers_ptr.add(b.buffer_index * max_block_frames),
                                            1,
                                        )
                                    }[0] == 0.0;
                                    flag.frames = frames_u16;
                                } else {
                                    flag.set_silent(false, frames_u16);
                                }
                            }
                        }
                    },
                }

                if status.is_bypass_declicking
                    && matches!(
                        status.status,
                        ProcessStatus::OutputsModified | ProcessStatus::OutputsModifiedWithMask(_)
                    )
                {
                    let wet_buffer = &self.bypass_gain_buffer[..self.max_block_frames];
                    let dry_buffer = &self.bypass_gain_buffer[self.max_block_frames..];

                    for (in_buf, out_buf) in inputs.iter().zip(outputs.iter_mut()) {
                        for (((&in_s, out_s), &gain_wet), &gain_dry) in in_buf
                            .iter()
                            .zip(out_buf.iter_mut())
                            .zip(wet_buffer.iter())
                            .zip(dry_buffer.iter())
                        {
                            *out_s = (*out_s * gain_wet) + (in_s * gain_dry);
                        }
                    }

                    for out_buf in outputs.iter_mut().skip(inputs.len()) {
                        for (out_s, &gain_wet) in out_buf.iter_mut().zip(wet_buffer.iter()) {
                            *out_s *= gain_wet;
                        }
                    }
                }
            }
        }
    }
}

/// # Safety
///
/// - `buffers_ptr` must be valid for reads and writes for all buffer indexes
///   referenced by `inserted_sum`, each spanning `max_block_frames` elements.
/// - `frames` must be less than or equal to `max_block_frames`.
/// - The buffer regions referenced by `inserted_sum` must not alias.
unsafe fn sum_inputs(
    inserted_sum: &InsertedSum,
    buffers_ptr: *mut f32,
    buffer_flags: &mut [BufferFlags],
    max_block_frames: usize,
    frames: usize,
) {
    let mut all_buffers_silent = true;

    // SAFETY: Buffer indexes are guaranteed non-overlapping by the allocator.
    let out_slice = unsafe {
        core::slice::from_raw_parts_mut(
            buffers_ptr.add(inserted_sum.output_buffer.buffer_index * max_block_frames),
            frames,
        )
    };

    if flag_mut(buffer_flags, inserted_sum.input_buffers[0].buffer_index).silent {
        if !flag_mut(buffer_flags, inserted_sum.output_buffer.buffer_index).silent {
            out_slice.fill(0.0);
        }
    } else {
        // SAFETY: Input and output buffer indexes are guaranteed distinct by the allocator.
        let in_slice = unsafe {
            core::slice::from_raw_parts_mut(
                buffers_ptr.add(inserted_sum.input_buffers[0].buffer_index * max_block_frames),
                frames,
            )
        };
        out_slice.copy_from_slice(in_slice);

        all_buffers_silent = false;
    }

    for buf_id in inserted_sum.input_buffers.iter().skip(1) {
        if flag_mut(buffer_flags, buf_id.buffer_index).silent {
            // Input channel is silent, no need to add it.
            continue;
        }

        all_buffers_silent = false;

        // SAFETY: Input buffer indexes are guaranteed distinct from the output buffer
        // index by the allocator.
        let in_slice = unsafe {
            core::slice::from_raw_parts_mut(
                buffers_ptr.add(buf_id.buffer_index * max_block_frames),
                frames,
            )
        };
        for (os, &is) in out_slice.iter_mut().zip(in_slice.iter()) {
            *os += is;
        }
    }

    flag_mut(buffer_flags, inserted_sum.output_buffer.buffer_index)
        .set_silent(all_buffers_silent, frames as u16);
}

#[inline]
fn flag_mut(buffer_flags: &mut [BufferFlags], buffer_index: usize) -> &mut BufferFlags {
    // SAFETY
    //
    // `buffer_index` is guaranteed to be valid because [`BufferAllocator`]
    // correctly counts the total number of buffers used, and therefore
    // `b.buffer_index` is guaranteed to be less than the value of
    // `num_buffers` that was passed into [`CompiledSchedule::new`].
    unsafe { buffer_flags.get_unchecked_mut(buffer_index) }
}

#[cfg(test)]
mod tests {
    use crate::{
        graph::{
            dummy_node::{DummyNode, DummyNodeConfig},
            AudioGraph, EdgeID,
        },
        FirewheelConfig,
    };
    use bevy_platform::collections::HashSet;
    use firewheel_core::channel_config::{ChannelConfig, ChannelCount};
    use firewheel_core::node::NodeError;

    use super::*;

    // Simplest graph compile test:
    //
    //  ┌───┐  ┌───┐
    //  │ 0 ┼──► 1 │
    //  └───┘  └───┘
    #[test]
    fn simplest_graph_compile_test() {
        let mut graph = AudioGraph::new(&FirewheelConfig {
            num_graph_inputs: ChannelCount::MONO,
            num_graph_outputs: ChannelCount::MONO,
            ..Default::default()
        });

        let node0 = graph.graph_in_node();
        let node1 = graph.graph_out_node();

        let edge0 = graph.connect(node0, node1, &[(0, 0)], false).unwrap()[0];

        let schedule = graph.compile_internal(128).unwrap();

        #[cfg(feature = "std")]
        dbg!(&schedule);

        assert_eq!(schedule.schedule.len(), 2);
        assert!(schedule.buffers.len() > 0);

        // First node must be node 0
        assert_eq!(schedule.schedule[0].id, node0);
        // Last node must be node 1
        assert_eq!(schedule.schedule[1].id, node1);

        verify_node(node0, &[], 0, &schedule, &graph);
        verify_node(node1, &[false], 0, &schedule, &graph);

        verify_edge(edge0, &graph, &schedule, None);
    }

    // Graph compile test 1:
    //
    //              ┌───┐  ┌───┐
    //         ┌────►   ┼──►   │
    //       ┌─┼─┐  ┼ 3 ┼──►   │
    //   ┌───►   │  └───┘  │   │  ┌───┐
    // ┌─┼─┐ │ 1 │  ┌───┐  │ 5 ┼──►   │
    // │   │ └─┬─┘  ┼   ┼──►   ┼──► 6 │
    // │ 0 │   └────► 4 ┼──►   │  └───┘
    // └─┬─┘        └───┘  │   │
    //   │   ┌───┐         │   │
    //   └───► 2 ┼─────────►   │
    //       └───┘         └───┘
    #[test]
    fn graph_compile_test_1() {
        let mut graph = AudioGraph::new(&FirewheelConfig {
            num_graph_inputs: ChannelCount::STEREO,
            num_graph_outputs: ChannelCount::STEREO,
            ..Default::default()
        });

        let node0 = graph.graph_in_node();
        let node1 = add_dummy_node(&mut graph, (1, 2)).unwrap();
        let node2 = add_dummy_node(&mut graph, (1, 1)).unwrap();
        let node3 = add_dummy_node(&mut graph, (2, 2)).unwrap();
        let node4 = add_dummy_node(&mut graph, (2, 2)).unwrap();
        let node5 = add_dummy_node(&mut graph, (5, 2)).unwrap();
        let node6 = graph.graph_out_node();

        let edge0 = graph.connect(node0, node1, &[(0, 0)], false).unwrap()[0];
        let edge1 = graph.connect(node0, node2, &[(1, 0)], false).unwrap()[0];
        let edge2 = graph.connect(node1, node3, &[(0, 0)], false).unwrap()[0];
        let edge3 = graph.connect(node1, node4, &[(1, 1)], false).unwrap()[0];
        let edge4 = graph.connect(node3, node5, &[(0, 0)], false).unwrap()[0];
        let edge5 = graph.connect(node3, node5, &[(1, 1)], false).unwrap()[0];
        let edge6 = graph.connect(node4, node5, &[(0, 2)], false).unwrap()[0];
        let edge7 = graph.connect(node4, node5, &[(1, 3)], false).unwrap()[0];
        let edge8 = graph.connect(node2, node5, &[(0, 4)], false).unwrap()[0];

        // Test adding multiple edges at once.
        let edges = graph
            .connect(node5, node6, &[(0, 0), (1, 1)], false)
            .unwrap();
        let edge9 = edges[0];
        let edge10 = edges[1];

        let schedule = graph.compile_internal(128).unwrap();

        #[cfg(feature = "std")]
        dbg!(&schedule);

        assert_eq!(schedule.schedule.len(), 7);
        // Node 5 needs at-least 7 buffers
        assert!(schedule.buffers.len() > 6);

        // First node must be node 0
        assert_eq!(schedule.schedule[0].id, node0);
        // Next two nodes must be 1 and 2
        assert!(schedule.schedule[1].id == node1 || schedule.schedule[1].id == node2);
        assert!(schedule.schedule[2].id == node1 || schedule.schedule[2].id == node2);
        // Next two nodes must be 3 and 4
        assert!(schedule.schedule[3].id == node3 || schedule.schedule[3].id == node4);
        assert!(schedule.schedule[4].id == node3 || schedule.schedule[4].id == node4);
        // Next node must be 5
        assert_eq!(schedule.schedule[5].id, node5);
        // Last node must be 6
        assert_eq!(schedule.schedule[6].id, node6);

        verify_node(node0, &[], 0, &schedule, &graph);
        verify_node(node1, &[false], 0, &schedule, &graph);
        verify_node(node2, &[false], 0, &schedule, &graph);
        verify_node(node3, &[false, true], 0, &schedule, &graph);
        verify_node(node4, &[true, false], 0, &schedule, &graph);
        verify_node(
            node5,
            &[false, false, false, false, false],
            0,
            &schedule,
            &graph,
        );
        verify_node(node6, &[false, false], 0, &schedule, &graph);

        verify_edge(edge0, &graph, &schedule, None);
        verify_edge(edge1, &graph, &schedule, None);
        verify_edge(edge2, &graph, &schedule, None);
        verify_edge(edge3, &graph, &schedule, None);
        verify_edge(edge4, &graph, &schedule, None);
        verify_edge(edge5, &graph, &schedule, None);
        verify_edge(edge6, &graph, &schedule, None);
        verify_edge(edge7, &graph, &schedule, None);
        verify_edge(edge8, &graph, &schedule, None);
        verify_edge(edge9, &graph, &schedule, None);
        verify_edge(edge10, &graph, &schedule, None);
    }

    // Graph compile test 2:
    //
    //           ┌───┐  ┌───┐
    //     ┌─────►   ┼──►   │
    //   ┌─┼─┐   ┼ 2 ┼  ┼   │  ┌───┐
    //   |   │   └───┘  │   ┼──►   │
    //   │ 0 │   ┌───┐  │ 4 ┼  ┼ 5 │
    //   └─┬─┘ ┌─►   ┼  ┼   │  └───┘
    //     └───●─► 3 ┼──►   │  ┌───┐
    //         │ └───┘  │   ┼──► 6 ┼
    //   ┌───┐ │        │   │  └───┘
    //   ┼ 1 ┼─●────────►   ┼
    //   └───┘          └───┘
    #[test]
    fn graph_compile_test_2() {
        let mut graph = AudioGraph::new(&FirewheelConfig {
            num_graph_inputs: ChannelCount::STEREO,
            num_graph_outputs: ChannelCount::STEREO,
            ..Default::default()
        });

        let node0 = graph.graph_in_node();
        let node1 = add_dummy_node(&mut graph, (1, 1)).unwrap();
        let node2 = add_dummy_node(&mut graph, (2, 2)).unwrap();
        let node3 = add_dummy_node(&mut graph, (2, 2)).unwrap();
        let node4 = add_dummy_node(&mut graph, (5, 4)).unwrap();
        let node5 = graph.graph_out_node();
        let node6 = add_dummy_node(&mut graph, (1, 1)).unwrap();

        let edge0 = graph.connect(node0, node2, &[(0, 0)], false).unwrap()[0];
        let edge1 = graph.connect(node0, node3, &[(0, 1)], false).unwrap()[0];
        let edge2 = graph.connect(node2, node4, &[(0, 0)], false).unwrap()[0];
        let edge3 = graph.connect(node3, node4, &[(1, 3)], false).unwrap()[0];
        let edge4 = graph.connect(node1, node3, &[(0, 1)], false).unwrap()[0];
        let edge5 = graph.connect(node1, node4, &[(0, 4)], false).unwrap()[0];
        let edge6 = graph.connect(node1, node3, &[(0, 0)], false).unwrap()[0];
        let edge7 = graph.connect(node4, node5, &[(0, 0)], false).unwrap()[0];
        let edge8 = graph.connect(node4, node6, &[(2, 0)], false).unwrap()[0];

        let schedule = graph.compile_internal(128).unwrap();

        #[cfg(feature = "std")]
        dbg!(&schedule);

        assert_eq!(schedule.schedule.len(), 7);
        // Node 4 needs at-least 8 buffers
        assert!(schedule.buffers.len() > 7);

        // First two nodes must be 0 and 1
        assert!(schedule.schedule[0].id == node0 || schedule.schedule[0].id == node1);
        assert!(schedule.schedule[1].id == node0 || schedule.schedule[1].id == node1);
        // Next two nodes must be 2 and 3
        assert!(schedule.schedule[2].id == node2 || schedule.schedule[2].id == node3);
        assert!(schedule.schedule[3].id == node2 || schedule.schedule[3].id == node3);
        // Next node must be 4
        assert_eq!(schedule.schedule[4].id, node4);
        // Last two nodes must be 5 and 6
        assert!(schedule.schedule[5].id == node5 || schedule.schedule[5].id == node6);
        assert!(schedule.schedule[6].id == node5 || schedule.schedule[6].id == node6);

        verify_edge(edge0, &graph, &schedule, None);
        verify_edge(edge1, &graph, &schedule, Some(0));
        verify_edge(edge2, &graph, &schedule, None);
        verify_edge(edge3, &graph, &schedule, None);
        verify_edge(edge4, &graph, &schedule, Some(0));
        verify_edge(edge5, &graph, &schedule, None);
        verify_edge(edge6, &graph, &schedule, None);
        verify_edge(edge7, &graph, &schedule, None);
        verify_edge(edge8, &graph, &schedule, None);

        verify_node(node0, &[], 0, &schedule, &graph);
        verify_node(node1, &[true], 0, &schedule, &graph);
        verify_node(node2, &[false, true], 0, &schedule, &graph);
        verify_node(node3, &[false, false], 1, &schedule, &graph);
        verify_node(
            node4,
            &[false, true, true, false, false],
            0,
            &schedule,
            &graph,
        );
        verify_node(node5, &[false, true], 0, &schedule, &graph);
        verify_node(node6, &[false], 0, &schedule, &graph);
    }

    fn add_dummy_node(
        graph: &mut AudioGraph,
        channel_config: impl Into<ChannelConfig>,
    ) -> Result<NodeID, NodeError> {
        graph.add_node(
            DummyNode,
            Some(DummyNodeConfig {
                channel_config: channel_config.into(),
            }),
        )
    }

    fn verify_node(
        node_id: NodeID,
        in_ports_that_should_clear: &[bool],
        num_sum_ins: usize,
        schedule: &CompiledSchedule,
        graph: &AudioGraph,
    ) {
        let node = graph.node_info(node_id).unwrap();
        let scheduled_node = schedule.schedule.iter().find(|&s| s.id == node_id).unwrap();

        let num_inputs = node.info.channel_config.num_inputs.get() as usize;
        let num_outputs = node.info.channel_config.num_outputs.get() as usize;

        assert_eq!(scheduled_node.id, node_id);
        assert_eq!(scheduled_node.input_buffers.len(), num_inputs);
        assert_eq!(scheduled_node.output_buffers.len(), num_outputs);
        assert_eq!(scheduled_node.sum_inputs.len(), num_sum_ins);

        assert_eq!(in_ports_that_should_clear.len(), num_inputs);

        for (buffer, should_clear) in scheduled_node
            .input_buffers
            .iter()
            .zip(in_ports_that_should_clear)
        {
            assert_eq!(buffer.should_clear, *should_clear);
        }

        let mut buffer_alias_check: HashSet<usize> = HashSet::default();

        for inserted_sum in scheduled_node.sum_inputs.iter() {
            buffer_alias_check.insert(inserted_sum.output_buffer.buffer_index);

            for in_buf in inserted_sum.input_buffers.iter() {
                assert!(buffer_alias_check.insert(in_buf.buffer_index));
            }

            buffer_alias_check.clear();
        }

        for buffer in scheduled_node.input_buffers.iter() {
            assert!(buffer_alias_check.insert(buffer.buffer_index));
        }

        for buffer in scheduled_node.output_buffers.iter() {
            assert!(buffer_alias_check.insert(buffer.buffer_index));
        }
    }

    fn verify_edge(
        edge_id: EdgeID,
        graph: &AudioGraph,
        schedule: &CompiledSchedule,
        inserted_sum_idx: Option<usize>,
    ) {
        let edge = graph.edge(edge_id).unwrap();

        let mut src_buffer_idx = None;
        let mut dst_buffer_idx = None;
        for node in schedule.schedule.iter() {
            if node.id == edge.src_node {
                src_buffer_idx = Some(node.output_buffers[edge.src_port as usize].buffer_index);
                if dst_buffer_idx.is_some() || inserted_sum_idx.is_some() {
                    break;
                }
            } else if node.id == edge.dst_node && inserted_sum_idx.is_none() {
                dst_buffer_idx = Some(node.input_buffers[edge.dst_port as usize].buffer_index);
                if src_buffer_idx.is_some() {
                    break;
                }
            }
        }

        let src_buffer_idx = src_buffer_idx.unwrap();

        if let Some(inserted_sum_idx) = inserted_sum_idx {
            // Assert that the source buffer appears in one of the sum's input.
            for node in schedule.schedule.iter() {
                if node.id == edge.dst_node {
                    let mut found = false;
                    for in_buf in node.sum_inputs[inserted_sum_idx].input_buffers.iter() {
                        if in_buf.buffer_index == src_buffer_idx {
                            found = true;
                            break;
                        }
                    }

                    assert!(found);

                    break;
                }
            }
        } else {
            let dst_buffer_idx = dst_buffer_idx.unwrap();

            assert_eq!(src_buffer_idx, dst_buffer_idx);
        }
    }

    #[test]
    fn cycle_detection() {
        let mut graph = AudioGraph::new(&FirewheelConfig {
            num_graph_inputs: ChannelCount::ZERO,
            num_graph_outputs: ChannelCount::STEREO,
            ..Default::default()
        });

        let node1 = add_dummy_node(&mut graph, (1, 1)).unwrap();
        let node2 = add_dummy_node(&mut graph, (2, 1)).unwrap();
        let node3 = add_dummy_node(&mut graph, (1, 1)).unwrap();

        // A zero input/output node shouldn't cause a cycle to be detected.
        let _node4 = add_dummy_node(&mut graph, (0, 0));

        graph.connect(node1, node2, &[(0, 0)], false).unwrap();
        graph.connect(node2, node3, &[(0, 0)], false).unwrap();
        let edge3 = graph.connect(node3, node1, &[(0, 0)], false).unwrap()[0];

        assert!(graph.cycle_detected());

        graph.disconnect_by_edge_id(edge3);

        assert!(!graph.cycle_detected());

        graph.connect(node3, node2, &[(0, 1)], false).unwrap();

        assert!(graph.cycle_detected());
    }
}
