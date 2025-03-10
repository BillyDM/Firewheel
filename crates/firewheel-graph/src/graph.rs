mod compiler;

use std::fmt::Debug;
use std::hash::Hash;

use ahash::AHashMap;
use firewheel_core::channel_config::{ChannelConfig, ChannelCount};
use firewheel_core::event::NodeEvent;
use firewheel_core::node::UpdateContext;
use firewheel_core::StreamInfo;
use smallvec::SmallVec;
use thunderdome::Arena;

use crate::error::{AddEdgeError, CompileGraphError};
use crate::FirewheelConfig;
use firewheel_core::node::{
    dummy::{DummyNode, DummyNodeConfig},
    AudioNode, AudioNodeInfo, AudioNodeInfoInner, Constructor, DynAudioNode, NodeID,
};

pub(crate) use self::compiler::{CompiledSchedule, NodeHeapData, ScheduleHeapData};

pub use self::compiler::{Edge, EdgeID, NodeEntry, PortIdx};

#[derive(Copy, Clone, Debug, Hash, Eq, PartialEq)]
struct EdgeHash {
    pub src_node: NodeID,
    pub dst_node: NodeID,
    pub src_port: PortIdx,
    pub dst_port: PortIdx,
}

/// The audio graph interface.
pub(crate) struct AudioGraph {
    nodes: Arena<NodeEntry>,
    edges: Arena<Edge>,
    existing_edges: AHashMap<EdgeHash, EdgeID>,

    graph_in_id: NodeID,
    graph_out_id: NodeID,
    needs_compile: bool,

    nodes_to_remove_from_schedule: Vec<NodeID>,
    active_nodes_to_remove: AHashMap<NodeID, NodeEntry>,
    nodes_to_call_update_method: Vec<NodeID>,
    event_queue_capacity: usize,
}

impl AudioGraph {
    pub fn new(config: &FirewheelConfig) -> Self {
        let mut nodes = Arena::with_capacity(config.initial_node_capacity as usize);

        let graph_in_config = DummyNodeConfig {
            channel_config: ChannelConfig {
                num_inputs: ChannelCount::ZERO,
                num_outputs: config.num_graph_inputs,
            },
        };
        let graph_out_config = DummyNodeConfig {
            channel_config: ChannelConfig {
                num_inputs: config.num_graph_outputs,
                num_outputs: ChannelCount::ZERO,
            },
        };

        let graph_in_id = NodeID(
            nodes.insert(NodeEntry::new(
                AudioNodeInfo::new()
                    .debug_name("graph_in")
                    .channel_config(graph_in_config.channel_config)
                    .uses_events(false)
                    .into(),
                Box::new(Constructor::new(DummyNode, Some(graph_in_config))),
            )),
        );
        nodes[graph_in_id.0].id = graph_in_id;

        let graph_out_id = NodeID(
            nodes.insert(NodeEntry::new(
                AudioNodeInfo::new()
                    .debug_name("graph_out")
                    .channel_config(graph_out_config.channel_config)
                    .uses_events(false)
                    .into(),
                Box::new(Constructor::new(DummyNode, Some(graph_out_config))),
            )),
        );
        nodes[graph_out_id.0].id = graph_out_id;

        Self {
            nodes,
            edges: Arena::with_capacity(config.initial_edge_capacity as usize),
            existing_edges: AHashMap::with_capacity(config.initial_edge_capacity as usize),
            graph_in_id,
            graph_out_id,
            needs_compile: true,
            nodes_to_remove_from_schedule: Vec::with_capacity(
                config.initial_node_capacity as usize,
            ),
            active_nodes_to_remove: AHashMap::with_capacity(config.initial_edge_capacity as usize),
            nodes_to_call_update_method: Vec::new(),
            event_queue_capacity: 0, // This will be overwritten later once activated.
        }
    }

    /// The ID of the graph input node
    pub fn graph_in_node(&self) -> NodeID {
        self.graph_in_id
    }

    /// The ID of the graph output node
    pub fn graph_out_node(&self) -> NodeID {
        self.graph_out_id
    }

    /// Add a node to the audio graph.
    pub fn add_node<T: AudioNode + 'static>(
        &mut self,
        node: T,
        config: Option<T::Configuration>,
    ) -> NodeID {
        let constructor = Constructor::new(node, config);
        let info: AudioNodeInfoInner = constructor.info().into();
        let call_update_method = info.call_update_method;

        let new_id = NodeID(
            self.nodes
                .insert(NodeEntry::new(info, Box::new(constructor))),
        );
        self.nodes[new_id.0].id = new_id;

        if call_update_method {
            self.nodes_to_call_update_method.push(new_id);
        }

        self.needs_compile = true;

        new_id
    }

    /// Add a node to the audio graph which implements the type-erased [`DynAudioNode`] trait.
    pub fn add_dyn_node<T: DynAudioNode + 'static>(&mut self, node: T) -> NodeID {
        let info: AudioNodeInfoInner = node.info().into();
        let call_update_method = info.call_update_method;

        let new_id = NodeID(self.nodes.insert(NodeEntry::new(info, Box::new(node))));
        self.nodes[new_id.0].id = new_id;

        if call_update_method {
            self.nodes_to_call_update_method.push(new_id);
        }

        self.needs_compile = true;

        new_id
    }

    /// Remove the given node from the audio graph.
    ///
    /// This will automatically remove all edges from the graph that
    /// were connected to this node.
    ///
    /// On success, this returns a list of all edges that were removed
    /// from the graph as a result of removing this node.
    ///
    /// This will return an error if a node with the given ID does not
    /// exist in the graph, or if the ID is of the graph input or graph
    /// output node.
    pub fn remove_node(&mut self, node_id: NodeID) -> Result<SmallVec<[EdgeID; 4]>, ()> {
        if node_id == self.graph_in_id || node_id == self.graph_out_id {
            return Err(());
        }

        let node_entry = self.nodes.remove(node_id.0).ok_or(())?;

        let mut removed_edges = SmallVec::new();

        for port_idx in 0..node_entry.info.channel_config.num_inputs.get() {
            removed_edges.append(&mut self.remove_edges_with_input_port(node_id, port_idx));
        }
        for port_idx in 0..node_entry.info.channel_config.num_outputs.get() {
            removed_edges.append(&mut self.remove_edges_with_output_port(node_id, port_idx));
        }

        self.nodes_to_remove_from_schedule.push(node_id);
        self.active_nodes_to_remove.insert(node_id, node_entry);

        self.needs_compile = true;

        Ok(removed_edges)
    }

    /// Get information about a node in the graph.
    pub fn node_info(&self, id: NodeID) -> Option<&NodeEntry> {
        self.nodes.get(id.0)
    }

    /// Get an immutable reference to the custom state of a node.
    pub fn node_state<T: 'static>(&self, id: NodeID) -> Option<&T> {
        self.nodes.get(id.0).and_then(|node_entry| {
            node_entry
                .info
                .custom_state
                .as_ref()
                .and_then(|s| s.downcast_ref::<T>())
        })
    }

    /// Get a mutable reference to the custom state of a node.
    pub fn node_state_mut<T: 'static>(&mut self, id: NodeID) -> Option<&mut T> {
        self.nodes.get_mut(id.0).and_then(|node_entry| {
            node_entry
                .info
                .custom_state
                .as_mut()
                .and_then(|s| s.downcast_mut::<T>())
        })
    }

    /// Get a list of all the existing nodes in the graph.
    pub fn nodes<'a>(&'a self) -> impl Iterator<Item = &'a NodeEntry> {
        self.nodes.iter().map(|(_, n)| n)
    }

    /// Get a list of all the existing edges in the graph.
    pub fn edges<'a>(&'a self) -> impl Iterator<Item = &'a Edge> {
        self.edges.iter().map(|(_, e)| e)
    }

    /// Set the number of input and output channels to and from the audio graph.
    ///
    /// Returns the list of edges that were removed.
    pub fn set_graph_channel_config(
        &mut self,
        channel_config: ChannelConfig,
    ) -> SmallVec<[EdgeID; 4]> {
        let mut removed_edges = SmallVec::new();

        let graph_in_node = self.nodes.get_mut(self.graph_in_id.0).unwrap();
        if channel_config.num_inputs != graph_in_node.info.channel_config.num_outputs {
            let old_num_inputs = graph_in_node.info.channel_config.num_outputs;
            graph_in_node.info.channel_config.num_outputs = channel_config.num_inputs;

            if channel_config.num_inputs < old_num_inputs {
                for port_idx in channel_config.num_inputs.get()..old_num_inputs.get() {
                    removed_edges.append(
                        &mut self.remove_edges_with_output_port(self.graph_in_id, port_idx),
                    );
                }
            }

            self.needs_compile = true;
        }

        let graph_out_node = self.nodes.get_mut(self.graph_in_id.0).unwrap();

        if channel_config.num_outputs != graph_out_node.info.channel_config.num_inputs {
            let old_num_outputs = graph_out_node.info.channel_config.num_inputs;
            graph_out_node.info.channel_config.num_inputs = channel_config.num_outputs;

            if channel_config.num_outputs < old_num_outputs {
                for port_idx in channel_config.num_outputs.get()..old_num_outputs.get() {
                    removed_edges.append(
                        &mut self.remove_edges_with_input_port(self.graph_out_id, port_idx),
                    );
                }
            }

            self.needs_compile = true;
        }

        removed_edges
    }

    /// Add connections (edges) between two nodes to the graph.
    ///
    /// * `src_node` - The ID of the source node.
    /// * `dst_node` - The ID of the destination node.
    /// * `ports_src_dst` - The port indices for each connection to make,
    /// where the first value in a tuple is the output port on `src_node`,
    /// and the second value in that tuple is the input port on `dst_node`.
    /// * `check_for_cycles` - If `true`, then this will run a check to
    /// see if adding these edges will create a cycle in the graph, and
    /// return an error if it does. Note, checking for cycles can be quite
    /// expensive, so avoid enabling this when calling this method many times
    /// in a row.
    ///
    /// If successful, then this returns a list of edge IDs in order.
    ///
    /// If this returns an error, then the audio graph has not been
    /// modified.
    pub fn connect(
        &mut self,
        src_node: NodeID,
        dst_node: NodeID,
        ports_src_dst: &[(PortIdx, PortIdx)],
        check_for_cycles: bool,
    ) -> Result<SmallVec<[EdgeID; 4]>, AddEdgeError> {
        let src_node_entry = self
            .nodes
            .get(src_node.0)
            .ok_or(AddEdgeError::SrcNodeNotFound(src_node))?;
        let dst_node_entry = self
            .nodes
            .get(dst_node.0)
            .ok_or(AddEdgeError::DstNodeNotFound(dst_node))?;

        if src_node.0 == dst_node.0 {
            return Err(AddEdgeError::CycleDetected);
        }

        for (src_port, dst_port) in ports_src_dst.iter().copied() {
            if src_port >= src_node_entry.info.channel_config.num_outputs.get() {
                return Err(AddEdgeError::OutPortOutOfRange {
                    node: src_node,
                    port_idx: src_port,
                    num_out_ports: src_node_entry.info.channel_config.num_outputs,
                });
            }
            if dst_port >= dst_node_entry.info.channel_config.num_inputs.get() {
                return Err(AddEdgeError::InPortOutOfRange {
                    node: dst_node,
                    port_idx: dst_port,
                    num_in_ports: dst_node_entry.info.channel_config.num_inputs,
                });
            }
        }

        let mut edge_ids = SmallVec::new();

        for (src_port, dst_port) in ports_src_dst.iter().copied() {
            if let Some(id) = self.existing_edges.get(&EdgeHash {
                src_node,
                src_port,
                dst_node,
                dst_port,
            }) {
                // The caller gave us more than one of the same edge.
                edge_ids.push(*id);
                continue;
            }

            let new_edge_id = EdgeID(self.edges.insert(Edge {
                id: EdgeID(thunderdome::Index::DANGLING),
                src_node,
                src_port,
                dst_node,
                dst_port,
            }));
            self.edges[new_edge_id.0].id = new_edge_id;
            self.existing_edges.insert(
                EdgeHash {
                    src_node,
                    src_port,
                    dst_node,
                    dst_port,
                },
                new_edge_id,
            );

            edge_ids.push(new_edge_id);
        }

        if check_for_cycles {
            if self.cycle_detected() {
                self.disconnect(src_node, dst_node, ports_src_dst);

                return Err(AddEdgeError::CycleDetected);
            }
        }

        self.needs_compile = true;

        Ok(edge_ids)
    }

    /// Remove connections (edges) between two nodes from the graph.
    ///
    /// * `src_node` - The ID of the source node.
    /// * `dst_node` - The ID of the destination node.
    /// * `ports_src_dst` - The port indices for each connection to make,
    /// where the first value in a tuple is the output port on `src_node`,
    /// and the second value in that tuple is the input port on `dst_node`.
    ///
    /// If none of the edges existed in the graph, then `false` will be
    /// returned.
    pub fn disconnect(
        &mut self,
        src_node: NodeID,
        dst_node: NodeID,
        ports_src_dst: &[(PortIdx, PortIdx)],
    ) -> bool {
        let mut any_removed = false;

        for (src_port, dst_port) in ports_src_dst.iter().copied() {
            if let Some(edge_id) = self.existing_edges.remove(&EdgeHash {
                src_node,
                src_port: src_port.into(),
                dst_node,
                dst_port: dst_port.into(),
            }) {
                self.disconnect_by_edge_id(edge_id);
                any_removed = true;
            }
        }

        any_removed
    }

    /// Remove all connections (edges) between two nodes in the graph.
    ///
    /// * `src_node` - The ID of the source node.
    /// * `dst_node` - The ID of the destination node.
    pub fn disconnect_all_between(
        &mut self,
        src_node: NodeID,
        dst_node: NodeID,
    ) -> SmallVec<[EdgeID; 4]> {
        let mut removed_edges = SmallVec::new();

        if !self.nodes.contains(src_node.0) || !self.nodes.contains(dst_node.0) {
            return removed_edges;
        };

        for (edge_id, edge) in self.edges.iter() {
            if edge.src_node == src_node && edge.dst_node == dst_node {
                removed_edges.push(EdgeID(edge_id));
            }
        }

        for &edge_id in removed_edges.iter() {
            self.disconnect_by_edge_id(edge_id);
        }

        removed_edges
    }

    /// Remove a connection (edge) via the edge's unique ID.
    ///
    /// If the edge did not exist in this graph, then `false` will be returned.
    pub fn disconnect_by_edge_id(&mut self, edge_id: EdgeID) -> bool {
        if let Some(edge) = self.edges.remove(edge_id.0) {
            self.existing_edges.remove(&EdgeHash {
                src_node: edge.src_node,
                src_port: edge.src_port,
                dst_node: edge.dst_node,
                dst_port: edge.dst_port,
            });

            self.needs_compile = true;

            true
        } else {
            false
        }
    }

    /// Get information about the given [Edge]
    pub fn edge(&self, edge_id: EdgeID) -> Option<&Edge> {
        self.edges.get(edge_id.0)
    }

    fn remove_edges_with_input_port(
        &mut self,
        node_id: NodeID,
        port_idx: PortIdx,
    ) -> SmallVec<[EdgeID; 4]> {
        let mut edges_to_remove = SmallVec::new();

        // Remove all existing edges which have this port.
        for (edge_id, edge) in self.edges.iter() {
            if edge.dst_node == node_id && edge.dst_port == port_idx {
                edges_to_remove.push(EdgeID(edge_id));
            }
        }

        for edge_id in edges_to_remove.iter() {
            self.disconnect_by_edge_id(*edge_id);
        }

        edges_to_remove
    }

    fn remove_edges_with_output_port(
        &mut self,
        node_id: NodeID,
        port_idx: PortIdx,
    ) -> SmallVec<[EdgeID; 4]> {
        let mut edges_to_remove = SmallVec::new();

        // Remove all existing edges which have this port.
        for (edge_id, edge) in self.edges.iter() {
            if edge.src_node == node_id && edge.src_port == port_idx {
                edges_to_remove.push(EdgeID(edge_id));
            }
        }

        for edge_id in edges_to_remove.iter() {
            self.disconnect_by_edge_id(*edge_id);
        }

        edges_to_remove
    }

    pub fn cycle_detected(&mut self) -> bool {
        compiler::cycle_detected(
            &mut self.nodes,
            &mut self.edges,
            self.graph_in_id,
            self.graph_out_id,
        )
    }

    pub(crate) fn needs_compile(&self) -> bool {
        self.needs_compile
    }

    pub(crate) fn on_schedule_send_failed(&mut self, failed_schedule: Box<ScheduleHeapData>) {
        self.needs_compile = true;

        for node in failed_schedule.new_node_processors.iter() {
            if let Some(node_entry) = &mut self.nodes.get_mut(node.id.0) {
                node_entry.activated = false;
            }
        }
    }

    pub(crate) fn node_capacity(&self) -> usize {
        self.nodes.capacity()
    }

    pub(crate) fn deactivate(&mut self) {
        for (_, entry) in self.nodes.iter_mut() {
            entry.activated = false;
        }
        self.needs_compile = true;
    }

    pub(crate) fn compile(
        &mut self,
        stream_info: &StreamInfo,
    ) -> Result<Box<ScheduleHeapData>, CompileGraphError> {
        let schedule = self.compile_internal(stream_info.max_block_frames.get() as usize)?;

        let mut new_node_processors = Vec::new();
        for (_, entry) in self.nodes.iter_mut() {
            if !entry.activated {
                entry.activated = true;

                let event_buffer_indices = if entry.info.uses_events {
                    Vec::with_capacity(self.event_queue_capacity)
                } else {
                    Vec::new()
                };

                new_node_processors.push(NodeHeapData {
                    id: entry.id,
                    processor: entry
                        .dyn_node
                        .processor(&stream_info, &mut entry.info.custom_state),
                    event_buffer_indices,
                });
            }
        }

        let mut nodes_to_remove = Vec::new();
        std::mem::swap(
            &mut self.nodes_to_remove_from_schedule,
            &mut nodes_to_remove,
        );

        let schedule_data = Box::new(ScheduleHeapData::new(
            schedule,
            nodes_to_remove,
            new_node_processors,
        ));

        self.needs_compile = false;

        log::debug!("compiled new audio graph: {:?}", &schedule_data);

        Ok(schedule_data)
    }

    fn compile_internal(
        &mut self,
        max_block_frames: usize,
    ) -> Result<CompiledSchedule, CompileGraphError> {
        assert!(max_block_frames > 0);

        compiler::compile(
            &mut self.nodes,
            &mut self.edges,
            self.graph_in_id,
            self.graph_out_id,
            max_block_frames,
        )
    }

    pub(crate) fn update(
        &mut self,
        stream_info: Option<&StreamInfo>,
        event_queue: &mut Vec<NodeEvent>,
    ) {
        let mut cull_list = false;
        for node_id in self.nodes_to_call_update_method.iter() {
            if let Some(node_entry) = self.nodes.get_mut(node_id.0) {
                node_entry.dyn_node.update(UpdateContext::new(
                    *node_id,
                    stream_info,
                    &mut node_entry.info.custom_state,
                    event_queue,
                ));
            } else {
                cull_list = true;
            }
        }

        if cull_list {
            self.nodes_to_call_update_method
                .retain(|node_id| self.nodes.contains(node_id.0));
        }
    }
}
