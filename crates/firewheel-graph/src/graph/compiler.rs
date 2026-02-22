use alloc::{collections::VecDeque, rc::Rc};
use firewheel_core::node::{AudioNodeInfoInner, DynAudioNode, NodeID};
use smallvec::SmallVec;
use thunderdome::Arena;

#[cfg(not(feature = "std"))]
use bevy_platform::prelude::{vec, Box, Vec};

use crate::error::CompileGraphError;

mod schedule;

pub use schedule::{CompiledSchedule, NodeHeapData, ScheduleHeapData};
use schedule::{InBufferAssignment, OutBufferAssignment, PreProcNode, ScheduledNode};

pub struct NodeEntry {
    pub id: NodeID,
    pub info: AudioNodeInfoInner,
    pub dyn_node: Box<dyn DynAudioNode>,
    pub processor_constructed: bool,
    /// The edges connected to this node's input ports.
    incoming: SmallVec<[Edge; 4]>,
    /// The edges connected to this node's output ports.
    outgoing: SmallVec<[Edge; 4]>,
}

impl NodeEntry {
    pub fn new(info: AudioNodeInfoInner, dyn_node: Box<dyn DynAudioNode>) -> Self {
        Self {
            id: NodeID::DANGLING,
            info,
            dyn_node,
            processor_constructed: false,
            incoming: SmallVec::new(),
            outgoing: SmallVec::new(),
        }
    }
}

/// The index of an input/output port on a particular node.
pub type PortIdx = u32;

/// A globally unique identifier for an [Edge].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EdgeID(pub(super) thunderdome::Index);

/// An [Edge] is a connection from source node and port to a
/// destination node and port.
#[derive(Copy, Clone, Debug, Hash, Eq, PartialEq)]
pub struct Edge {
    pub id: EdgeID,
    /// The ID of the source node used by this edge.
    pub src_node: NodeID,
    /// The ID of the source port used by this edge.
    pub src_port: PortIdx,
    /// The ID of the destination node used by this edge.
    pub dst_node: NodeID,
    /// The ID of the destination port used by this edge.
    pub dst_port: PortIdx,
}

/// A reference to an abstract buffer during buffer allocation.
#[derive(Debug, Clone, Copy)]
struct BufferRef {
    /// The index of the buffer
    idx: usize,
    /// The generation, or the nth time this buffer has
    /// been assigned to a different edge in the graph.
    generation: usize,
}

/// An allocator for managing and reusing [BufferRef]s.
#[derive(Debug, Clone)]
struct BufferAllocator {
    /// A list of free buffers that may be reallocated
    free_list: Vec<BufferRef>,
    /// The maximum number of buffers used
    count: usize,
}

impl BufferAllocator {
    /// Create a new allocator, `num_types` defines the number
    /// of buffer types we may allocate.
    fn new(initial_capacity: usize) -> Self {
        Self {
            free_list: Vec::with_capacity(initial_capacity),
            count: 0,
        }
    }

    /// Acquire a new buffer
    fn acquire(&mut self) -> Rc<BufferRef> {
        let entry = self.free_list.pop().unwrap_or_else(|| {
            let idx = self.count;
            self.count += 1;
            BufferRef { idx, generation: 0 }
        });
        Rc::new(BufferRef {
            idx: entry.idx,
            generation: entry.generation,
        })
    }

    /// Release a BufferRef
    fn release(&mut self, buffer_ref: Rc<BufferRef>) {
        if Rc::strong_count(&buffer_ref) == 1 {
            self.free_list.push(BufferRef {
                idx: buffer_ref.idx,
                generation: buffer_ref.generation + 1,
            });
        }
    }

    /// Consume the allocator to return the maximum number of buffers used
    fn num_buffers(self) -> usize {
        self.count
    }
}

/// Main compilation algorithm
pub fn compile(
    nodes: &mut Arena<NodeEntry>,
    edges: &mut Arena<Edge>,
    graph_in_id: NodeID,
    graph_out_id: NodeID,
    max_block_frames: usize,
) -> Result<CompiledSchedule, CompileGraphError> {
    Ok(
        GraphIR::preprocess(nodes, edges, graph_in_id, graph_out_id, max_block_frames)
            .sort_topologically(true)?
            .solve_buffer_requirements()?
            .merge(),
    )
}

pub fn cycle_detected<'a>(
    nodes: &'a mut Arena<NodeEntry>,
    edges: &'a mut Arena<Edge>,
    graph_in_id: NodeID,
    graph_out_id: NodeID,
) -> bool {
    matches!(
        GraphIR::preprocess(nodes, edges, graph_in_id, graph_out_id, 0).sort_topologically(false),
        Err(CompileGraphError::CycleDetected)
    )
}

/// Internal IR used by the compiler algorithm. Built incrementally
/// via the compiler passes.
struct GraphIR<'a> {
    nodes: &'a mut Arena<NodeEntry>,
    edges: &'a mut Arena<Edge>,

    /// Nodes with zero inputs and outputs are "pre process nodes" that get
    /// processed before all other nodes.
    pre_proc_nodes: Vec<PreProcNode>,
    /// The topologically sorted schedule of the graph. Built internally.
    schedule: Vec<ScheduledNode>,
    /// The maximum number of buffers used.
    max_num_buffers: usize,

    graph_in_id: NodeID,
    graph_out_id: NodeID,
    max_in_buffers: usize,
    max_out_buffers: usize,
    max_block_frames: usize,
}

impl<'a> GraphIR<'a> {
    /// Construct a [GraphIR] instance from lists of nodes and edges, building
    /// up the adjacency table and creating an empty schedule.
    fn preprocess(
        nodes: &'a mut Arena<NodeEntry>,
        edges: &'a mut Arena<Edge>,
        graph_in_id: NodeID,
        graph_out_id: NodeID,
        max_block_frames: usize,
    ) -> Self {
        assert!(nodes.contains(graph_in_id.0));
        assert!(nodes.contains(graph_out_id.0));

        for (_, node) in nodes.iter_mut() {
            node.incoming.clear();
            node.outgoing.clear();
        }

        for (_, edge) in edges.iter() {
            nodes[edge.src_node.0].outgoing.push(*edge);
            nodes[edge.dst_node.0].incoming.push(*edge);

            debug_assert_ne!(edge.src_node, graph_out_id);
            debug_assert_ne!(edge.dst_node, graph_in_id);
        }

        Self {
            nodes,
            edges,
            pre_proc_nodes: vec![],
            schedule: vec![],
            max_num_buffers: 0,
            graph_in_id,
            graph_out_id,
            max_in_buffers: 0,
            max_out_buffers: 0,
            max_block_frames,
        }
    }

    /// Sort the nodes topologically using Kahn's algorithm.
    /// <https://www.geeksforgeeks.org/topological-sorting-indegree-based-solution/>
    fn sort_topologically(mut self, build_schedule: bool) -> Result<Self, CompileGraphError> {
        let mut in_degree = vec![0i32; self.nodes.capacity()];
        let mut queue = VecDeque::with_capacity(self.nodes.len());

        if build_schedule {
            self.schedule.reserve(self.nodes.len());
        }

        let mut num_visited = 0;

        // Calculate in-degree of each vertex
        for (_, node_entry) in self.nodes.iter() {
            for edge in node_entry.outgoing.iter() {
                in_degree[edge.dst_node.0.slot() as usize] += 1;
            }
        }

        // Make sure that the graph in node is the first entry in the
        // schedule. Otherwise a different root node could overwrite
        // the buffers assigned to the graph in node.
        queue.push_back(self.graph_in_id.0.slot());

        // Enqueue all other nodes with 0 in-degree
        for (_, node_entry) in self.nodes.iter() {
            if node_entry.incoming.is_empty() && node_entry.id.0.slot() != self.graph_in_id.0.slot()
            {
                // If the number of inputs and outputs on a node is zero, then it
                // is a "pre process" node.
                if node_entry.info.channel_config.is_empty() {
                    self.pre_proc_nodes.push(PreProcNode {
                        id: node_entry.id,
                        debug_name: node_entry.info.debug_name,
                    });

                    num_visited += 1;
                } else {
                    queue.push_back(node_entry.id.0.slot());
                }
            }
        }

        // BFS traversal
        while let Some(node_slot) = queue.pop_front() {
            num_visited += 1;

            let (_, node_entry) = self.nodes.get_by_slot(node_slot).unwrap();

            // Reduce in-degree of adjacent nodes
            for edge in node_entry.outgoing.iter() {
                in_degree[edge.dst_node.0.slot() as usize] -= 1;

                // If in-degree becomes 0, enqueue it
                if in_degree[edge.dst_node.0.slot() as usize] == 0 {
                    queue.push_back(edge.dst_node.0.slot());
                }
            }

            if build_schedule && node_slot != self.graph_out_id.0.slot() {
                self.schedule.push(ScheduledNode::new(
                    node_entry.id,
                    node_entry.info.debug_name,
                ));
            }
        }

        if build_schedule {
            // Make sure that the graph out node is the last entry in the
            // schedule by waiting to push it after all other nodes have
            // been pushed. Otherwise a different leaf node could overwrite
            // the buffers assigned to the graph out node.
            self.schedule
                .push(ScheduledNode::new(self.graph_out_id, "graph_out"));
        }

        // If not all vertices are visited, cycle
        if num_visited != self.nodes.len() {
            return Err(CompileGraphError::CycleDetected);
        }

        Ok(self)
    }

    fn solve_buffer_requirements(mut self) -> Result<Self, CompileGraphError> {
        let mut allocator = BufferAllocator::new(64);
        let mut assignment_table: Arena<Rc<BufferRef>> =
            Arena::with_capacity(self.edges.capacity());
        let mut buffers_to_release: Vec<Rc<BufferRef>> = Vec::with_capacity(64);

        for entry in &mut self.schedule {
            // Collect the inputs to the algorithm, the incoming/outgoing edges of this node.

            let node_entry = &self.nodes[entry.id.0];

            let num_inputs = node_entry.info.channel_config.num_inputs.get() as usize;
            let num_outputs = node_entry.info.channel_config.num_outputs.get() as usize;

            buffers_to_release.clear();
            if buffers_to_release.capacity() < num_inputs + num_outputs {
                buffers_to_release
                    .reserve(num_inputs + num_outputs - buffers_to_release.capacity());
            }

            entry.input_buffers.reserve_exact(num_inputs);
            entry.output_buffers.reserve_exact(num_outputs);

            for port_idx in 0..num_inputs as u32 {
                let edges: SmallVec<[&Edge; 4]> = node_entry
                    .incoming
                    .iter()
                    .filter(|edge| edge.dst_port == port_idx)
                    .collect();

                entry
                    .in_connected_mask
                    .set_channel(port_idx as usize, !edges.is_empty());

                if edges.is_empty() {
                    // Case 1: The port is an input and it is unconnected. Acquire a buffer, and
                    //         assign it. The buffer must be cleared. Release the buffer once the
                    //         node assignments are done.
                    let buffer = allocator.acquire();
                    entry.input_buffers.push(InBufferAssignment {
                        buffer_index: buffer.idx,
                        //generation: buffer.generation,
                        should_clear: true,
                    });
                    buffers_to_release.push(buffer);
                } else if edges.len() == 1 {
                    // Case 2: The port is an input, and has exactly one incoming edge. Lookup the
                    //         corresponding buffer and assign it. Buffer should not be cleared.
                    //         Release the buffer once the node assignments are done.
                    let buffer = assignment_table
                        .remove(edges[0].id.0)
                        .expect("No buffer assigned to edge!");
                    entry.input_buffers.push(InBufferAssignment {
                        buffer_index: buffer.idx,
                        //generation: buffer.generation,
                        should_clear: false,
                    });
                    buffers_to_release.push(buffer);
                } else {
                    // Case 3: The port is an input with multiple incoming edges. Compute the
                    //         summing point, and assign the input buffer assignment to the output
                    //         of the summing point.

                    let sum_buffer = allocator.acquire();
                    let sum_output = OutBufferAssignment {
                        buffer_index: sum_buffer.idx,
                        //generation: sum_buffer.generation,
                    };

                    // The sum inputs are the corresponding output buffers of the incoming edges.
                    let sum_inputs = edges
                        .iter()
                        .map(|edge| {
                            let buf = assignment_table
                                .remove(edge.id.0)
                                .expect("No buffer assigned to edge!");
                            let assignment = InBufferAssignment {
                                buffer_index: buf.idx,
                                //generation: buf.generation,
                                should_clear: false,
                            };
                            allocator.release(buf);
                            assignment
                        })
                        .collect();

                    entry.sum_inputs.push(InsertedSum {
                        input_buffers: sum_inputs,
                        output_buffer: sum_output,
                    });

                    // This node's input buffer is the sum output buffer. Release it once the node
                    // assignments are done.
                    entry.input_buffers.push(InBufferAssignment {
                        buffer_index: sum_output.buffer_index,
                        //generation: sum_output.generation,
                        should_clear: false,
                    });

                    buffers_to_release.push(sum_buffer);
                }
            }

            for port_idx in 0..num_outputs as u32 {
                let edges: SmallVec<[&Edge; 4]> = node_entry
                    .outgoing
                    .iter()
                    .filter(|edge| edge.src_port == port_idx)
                    .collect();

                entry
                    .out_connected_mask
                    .set_channel(port_idx as usize, !edges.is_empty());

                if edges.is_empty() {
                    // Case 1: The port is an output and it is unconnected. Acquire a buffer and
                    //         assign it. The buffer does not need to be cleared. Release the
                    //         buffer once the node assignments are done.
                    let buffer = allocator.acquire();
                    entry.output_buffers.push(OutBufferAssignment {
                        buffer_index: buffer.idx,
                        //generation: buffer.generation,
                    });
                    buffers_to_release.push(buffer);
                } else {
                    // Case 2: The port is an output. Acquire a buffer, and add to the assignment
                    //         table with any corresponding edge IDs. For each edge, update the
                    //         assigned buffer table. Buffer should not be cleared or released.
                    let buffer = allocator.acquire();
                    for edge in &edges {
                        assignment_table.insert_at(edge.id.0, Rc::clone(&buffer));
                    }
                    entry.output_buffers.push(OutBufferAssignment {
                        buffer_index: buffer.idx,
                        //generation: buffer.generation,
                    });
                }
            }

            for buffer in buffers_to_release.drain(..) {
                allocator.release(buffer);
            }

            self.max_in_buffers = self.max_in_buffers.max(num_inputs);
            self.max_out_buffers = self.max_out_buffers.max(num_outputs);
        }

        self.max_num_buffers = allocator.num_buffers();
        Ok(self)
    }

    /// Merge the GraphIR into a [CompiledSchedule].
    fn merge(self) -> CompiledSchedule {
        CompiledSchedule::new(
            self.pre_proc_nodes,
            self.schedule,
            self.max_num_buffers,
            self.max_block_frames,
            self.graph_in_id,
        )
    }
}

#[derive(Debug, Clone)]
struct InsertedSum {
    input_buffers: SmallVec<[InBufferAssignment; 4]>,
    output_buffer: OutBufferAssignment,
}
