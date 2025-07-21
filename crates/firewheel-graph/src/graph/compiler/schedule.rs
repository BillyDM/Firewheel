use arrayvec::ArrayVec;
use core::fmt::Debug;
use smallvec::SmallVec;
use thunderdome::Arena;

use firewheel_core::{
    channel_config::MAX_CHANNELS,
    node::{AudioNodeProcessor, ProcBuffers, ProcessStatus, NUM_SCRATCH_BUFFERS},
    SilenceMask,
};

use super::{InsertedSum, NodeID};

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

    pub sum_inputs: Vec<InsertedSum>,
}

impl ScheduledNode {
    pub fn new(id: NodeID, debug_name: &'static str) -> Self {
        Self {
            id,
            debug_name,
            input_buffers: SmallVec::new(),
            output_buffers: SmallVec::new(),
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

        /*
        if !self.input_buffers.is_empty() {
            write!(f, " | in_gen: [")?;

            write!(f, "{}", self.input_buffers[0].generation)?;
            for b in self.input_buffers.iter().skip(1) {
                write!(f, ", {}", b.generation)?;
            }

            write!(f, "]")?;
        }

        if !self.output_buffers.is_empty() {
            write!(f, " | out_gen: [")?;

            write!(f, "{}", self.output_buffers[0].generation)?;
            for b in self.output_buffers.iter().skip(1) {
                write!(f, ", {}", b.generation)?;
            }

            write!(f, "]")?;
        }
        */

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
    /*
    /// Buffers are reused, the "generation" represents
    /// how many times this buffer has been used before
    /// this assignment. Kept for debugging and visualization.
    pub generation: usize,
    */
}

/// Represents a single buffer assigned to an output port
#[derive(Copy, Clone, Debug)]
pub(super) struct OutBufferAssignment {
    /// The index of the buffer assigned
    pub buffer_index: usize,
    /*
    /// Buffers are reused, the "generation" represents
    /// how many times this buffer has been used before
    /// this assignment. Kept for debugging and visualization.
    pub generation: usize,
    */
}

pub struct NodeHeapData {
    pub id: NodeID,
    pub processor: Box<dyn AudioNodeProcessor>,
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

/// A [CompiledSchedule] is the output of the graph compiler.
pub struct CompiledSchedule {
    schedule: Vec<ScheduledNode>,

    buffers: Vec<f32>,
    buffer_silence_flags: Vec<bool>,
    num_buffers: usize,
    max_block_frames: usize,
    graph_in_node_id: NodeID,
}

impl Debug for CompiledSchedule {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        writeln!(f, "CompiledSchedule {{")?;

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
        schedule: Vec<ScheduledNode>,
        num_buffers: usize,
        max_block_frames: usize,
        graph_in_node_id: NodeID,
    ) -> Self {
        let mut buffers = Vec::new();
        buffers.reserve_exact(num_buffers * max_block_frames);
        buffers.resize(num_buffers * max_block_frames, 0.0);

        Self {
            schedule,
            buffers,
            buffer_silence_flags: vec![false; num_buffers],
            num_buffers,
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

        let graph_in_node = self.schedule.first().unwrap();

        let mut inputs: ArrayVec<&mut [f32], MAX_CHANNELS> = ArrayVec::new();

        let fill_input_len = num_stream_inputs.min(graph_in_node.output_buffers.len());

        for i in 0..fill_input_len {
            inputs.push(buffer_slice_mut(
                &self.buffers,
                graph_in_node.output_buffers[i].buffer_index,
                self.max_block_frames,
                frames,
            ));
        }

        let silence_mask = (fill_inputs)(inputs.as_mut_slice());

        for i in 0..fill_input_len {
            let buffer_index = graph_in_node.output_buffers[i].buffer_index;
            *silence_flag_mut(&mut self.buffer_silence_flags, buffer_index) =
                silence_mask.is_channel_silent(i);
        }

        if fill_input_len < graph_in_node.output_buffers.len() {
            for b in graph_in_node.output_buffers.iter().skip(fill_input_len) {
                let buf_slice =
                    buffer_slice_mut(&self.buffers, b.buffer_index, self.max_block_frames, frames);
                buf_slice.fill(0.0);

                *silence_flag_mut(&mut self.buffer_silence_flags, b.buffer_index) = true;
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

        let graph_out_node = self.schedule.last().unwrap();

        let mut outputs: ArrayVec<&[f32], MAX_CHANNELS> = ArrayVec::new();

        let mut silence_mask = SilenceMask::NONE_SILENT;

        let read_output_len = num_stream_outputs.min(graph_out_node.input_buffers.len());

        for i in 0..read_output_len {
            let buffer_index = graph_out_node.input_buffers[i].buffer_index;

            if *silence_flag_mut(&mut self.buffer_silence_flags, buffer_index) {
                silence_mask.set_channel(i, true);
            }

            outputs.push(buffer_slice_mut(
                &self.buffers,
                buffer_index,
                self.max_block_frames,
                frames,
            ));
        }

        (read_outputs)(outputs.as_slice(), silence_mask);
    }

    pub fn process<'a, 'b>(
        &mut self,
        frames: usize,
        scratch_buffers: &'a mut [&'b mut [f32]; NUM_SCRATCH_BUFFERS],
        mut process: impl FnMut(NodeID, SilenceMask, SilenceMask, ProcBuffers) -> ProcessStatus,
    ) {
        let frames = frames.min(self.max_block_frames);

        let mut inputs: ArrayVec<&[f32], MAX_CHANNELS> = ArrayVec::new();
        let mut outputs: ArrayVec<&mut [f32], MAX_CHANNELS> = ArrayVec::new();

        for scheduled_node in self.schedule.iter() {
            if scheduled_node.id == self.graph_in_node_id {
                continue;
            }

            for inserted_sum in scheduled_node.sum_inputs.iter() {
                sum_inputs(
                    inserted_sum,
                    &self.buffers,
                    &mut self.buffer_silence_flags,
                    self.max_block_frames,
                    frames,
                );
            }

            let mut in_silence_mask = SilenceMask::NONE_SILENT;
            let mut out_silence_mask = SilenceMask::NONE_SILENT;

            inputs.clear();
            outputs.clear();

            for (i, b) in scheduled_node.input_buffers.iter().enumerate() {
                let buf =
                    buffer_slice_mut(&self.buffers, b.buffer_index, self.max_block_frames, frames);
                let s = silence_flag_mut(&mut self.buffer_silence_flags, b.buffer_index);

                if b.should_clear {
                    buf[..frames].fill(0.0);
                    *s = true;
                }

                if *s {
                    in_silence_mask.set_channel(i, true);
                }

                inputs.push(buf);
            }

            for (i, b) in scheduled_node.output_buffers.iter().enumerate() {
                let buf =
                    buffer_slice_mut(&self.buffers, b.buffer_index, self.max_block_frames, frames);
                let s = silence_flag_mut(&mut self.buffer_silence_flags, b.buffer_index);

                if *s {
                    out_silence_mask.set_channel(i, true);
                }

                outputs.push(buf);
            }

            let status = (process)(
                scheduled_node.id,
                in_silence_mask,
                out_silence_mask,
                ProcBuffers {
                    inputs: inputs.as_slice(),
                    outputs: outputs.as_mut_slice(),
                    scratch_buffers,
                },
            );

            let clear_buffer = |buffer_index: usize, silence_flag: &mut bool| {
                if !*silence_flag {
                    buffer_slice_mut(&self.buffers, buffer_index, self.max_block_frames, frames)
                        .fill(0.0);
                    *silence_flag = true;
                }
            };

            match status {
                ProcessStatus::ClearAllOutputs => {
                    // Clear output buffers which need cleared.
                    for b in scheduled_node.output_buffers.iter() {
                        let s = silence_flag_mut(&mut self.buffer_silence_flags, b.buffer_index);

                        clear_buffer(b.buffer_index, s);
                    }
                }
                ProcessStatus::Bypass => {
                    for (in_buf, out_buf) in scheduled_node
                        .input_buffers
                        .iter()
                        .zip(scheduled_node.output_buffers.iter())
                    {
                        let in_s =
                            *silence_flag_mut(&mut self.buffer_silence_flags, in_buf.buffer_index);
                        let out_s =
                            silence_flag_mut(&mut self.buffer_silence_flags, out_buf.buffer_index);

                        if in_s {
                            clear_buffer(out_buf.buffer_index, out_s);
                        } else {
                            let in_buf_slice = buffer_slice_mut(
                                &self.buffers,
                                in_buf.buffer_index,
                                self.max_block_frames,
                                frames,
                            );
                            let out_buf_slice = buffer_slice_mut(
                                &self.buffers,
                                out_buf.buffer_index,
                                self.max_block_frames,
                                frames,
                            );

                            out_buf_slice.copy_from_slice(in_buf_slice);
                            *out_s = false;
                        }
                    }

                    for b in scheduled_node
                        .output_buffers
                        .iter()
                        .skip(scheduled_node.input_buffers.len())
                    {
                        let s = silence_flag_mut(&mut self.buffer_silence_flags, b.buffer_index);

                        clear_buffer(b.buffer_index, s);
                    }
                }
                ProcessStatus::OutputsModified { out_silence_mask } => {
                    for (i, b) in scheduled_node.output_buffers.iter().enumerate() {
                        *silence_flag_mut(&mut self.buffer_silence_flags, b.buffer_index) =
                            out_silence_mask.is_channel_silent(i);
                    }
                }
            }
        }
    }
}

fn sum_inputs(
    inserted_sum: &InsertedSum,
    buffers: &Vec<f32>,
    buffer_silence_flags: &mut [bool],
    max_block_frames: usize,
    frames: usize,
) {
    let mut all_buffers_silent = true;

    let out_slice = buffer_slice_mut(
        buffers,
        inserted_sum.output_buffer.buffer_index,
        max_block_frames,
        frames,
    );

    if *silence_flag_mut(
        buffer_silence_flags,
        inserted_sum.input_buffers[0].buffer_index,
    ) {
        if !*silence_flag_mut(
            buffer_silence_flags,
            inserted_sum.output_buffer.buffer_index,
        ) {
            buffer_slice_mut(
                buffers,
                inserted_sum.output_buffer.buffer_index,
                max_block_frames,
                frames,
            )
            .fill(0.0);
        }
    } else {
        let in_slice = buffer_slice_mut(
            buffers,
            inserted_sum.input_buffers[0].buffer_index,
            max_block_frames,
            frames,
        );
        out_slice.copy_from_slice(in_slice);

        all_buffers_silent = false;
    }

    for buf_id in inserted_sum.input_buffers.iter().skip(1) {
        if *silence_flag_mut(buffer_silence_flags, buf_id.buffer_index) {
            // Input channel is silent, no need to add it.
            continue;
        }

        all_buffers_silent = false;

        let in_slice = buffer_slice_mut(buffers, buf_id.buffer_index, max_block_frames, frames);
        for (os, &is) in out_slice.iter_mut().zip(in_slice.iter()) {
            *os += is;
        }
    }

    *silence_flag_mut(
        buffer_silence_flags,
        inserted_sum.output_buffer.buffer_index,
    ) = all_buffers_silent;
}

#[inline]
#[allow(clippy::mut_from_ref)]
fn buffer_slice_mut<'a>(
    buffers: &'a Vec<f32>,
    buffer_index: usize,
    max_block_frames: usize,
    frames: usize,
) -> &'a mut [f32] {
    // SAFETY
    //
    // `buffer_index` is gauranteed to be valid because [`BufferAllocator`]
    // correctly counts the total number of buffers used, and therefore
    // `b.buffer_index` is gauranteed to be less than the value of
    // `num_buffers` that was passed into [`CompiledSchedule::new`].
    //
    // The methods calling this function make sure that `frames <= max_block_frames`,
    // and `buffers` was initialized with a length of `num_buffers * max_block_frames`
    // in the constructor. And because `buffer_index` is gauranteed to be less than
    // `num_buffers`, this slice will always point to a valid range.
    //
    // Due to the way [`GraphIR::solve_buffer_requirements`] works, no
    // two buffer indexes in a single `ScheduledNode` can alias. (A buffer
    // index can only be reused after `allocator.release()` is called for
    // that buffer, and that method only gets called *after* all buffer
    // assignments have already been populated for that `ScheduledNode`.)
    // Also, `self` is borrowed mutably here, ensuring that the caller cannot
    // call any other method on [`CompiledSchedule`] while those buffers are
    // still borrowed.
    unsafe {
        core::slice::from_raw_parts_mut(
            (buffers.as_ptr() as *mut f32).add(buffer_index * max_block_frames),
            frames,
        )
    }
}

#[inline]
fn silence_flag_mut<'a>(buffer_silence_flags: &'a mut [bool], buffer_index: usize) -> &'a mut bool {
    // SAFETY
    //
    // `buffer_index` is gauranteed to be valid because [`BufferAllocator`]
    // correctly counts the total number of buffers used, and therefore
    // `b.buffer_index` is gauranteed to be less than the value of
    // `num_buffers` that was passed into [`CompiledSchedule::new`].
    unsafe { buffer_silence_flags.get_unchecked_mut(buffer_index) }
}

#[cfg(test)]
mod tests {
    use bevy_platform::collections::HashSet;
    use firewheel_core::{
        channel_config::{ChannelConfig, ChannelCount},
        node::dummy::{DummyNode, DummyNodeConfig},
    };

    use crate::{
        graph::{AudioGraph, EdgeID},
        FirewheelConfig,
    };

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
        let node1 = add_dummy_node(&mut graph, (1, 2));
        let node2 = add_dummy_node(&mut graph, (1, 1));
        let node3 = add_dummy_node(&mut graph, (2, 2));
        let node4 = add_dummy_node(&mut graph, (2, 2));
        let node5 = add_dummy_node(&mut graph, (5, 2));
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
        let node1 = add_dummy_node(&mut graph, (1, 1));
        let node2 = add_dummy_node(&mut graph, (2, 2));
        let node3 = add_dummy_node(&mut graph, (2, 2));
        let node4 = add_dummy_node(&mut graph, (5, 4));
        let node5 = graph.graph_out_node();
        let node6 = add_dummy_node(&mut graph, (1, 1));

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

    fn add_dummy_node(graph: &mut AudioGraph, channel_config: impl Into<ChannelConfig>) -> NodeID {
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

        let node1 = add_dummy_node(&mut graph, (1, 1));
        let node2 = add_dummy_node(&mut graph, (2, 1));
        let node3 = add_dummy_node(&mut graph, (1, 1));

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
