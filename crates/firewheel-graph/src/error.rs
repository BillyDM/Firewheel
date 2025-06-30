use core::error::Error;
use firewheel_core::{channel_config::ChannelCount, node::NodeID};

use crate::graph::{Edge, EdgeID, PortIdx};

/// An error occurred while attempting to add an edge to the graph.
#[derive(Debug, thiserror::Error)]
pub enum AddEdgeError {
    /// The given source node was not found in the graph.
    #[error("Could not add edge: could not find source node with ID {0:?}")]
    SrcNodeNotFound(NodeID),
    /// The given destination node was not found in the graph.
    #[error("Could not add edge: could not find destination node with ID {0:?}")]
    DstNodeNotFound(NodeID),
    /// The given input port index is out of range.
    #[error("Input port idx {port_idx:?} is out of range on node {node:?} with {num_in_ports:?} input ports")]
    InPortOutOfRange {
        node: NodeID,
        port_idx: PortIdx,
        num_in_ports: ChannelCount,
    },
    /// The given output port index is out of range.
    #[error("Output port idx {port_idx:?} is out of range on node {node:?} with {num_out_ports:?} output ports")]
    OutPortOutOfRange {
        node: NodeID,
        port_idx: PortIdx,
        num_out_ports: ChannelCount,
    },
    /// This edge would have created a cycle in the graph.
    #[error("Could not add edge: cycle was detected")]
    CycleDetected,
}

/// An error occurred while attempting to compile the audio graph
/// into a schedule.
#[derive(Debug, thiserror::Error)]
pub enum CompileGraphError {
    /// A cycle was detected in the graph.
    #[error("Failed to compile audio graph: a cycle was detected")]
    CycleDetected,
    /// The input data contained an edge referring to a non-existing node.
    #[error("Failed to compile audio graph: input data contains an edge {0:?} referring to a non-existing node {1:?}")]
    NodeOnEdgeNotFound(Edge, NodeID),
    /// The input data contained multiple nodes with the same ID.
    #[error(
        "Failed to compile audio graph: input data contains multiple nodes with the same ID {0:?}"
    )]
    NodeIDNotUnique(NodeID),
    /// The input data contained multiple edges with the same ID.
    #[error(
        "Failed to compile audio graph: input data contains multiple edges with the same ID {0:?}"
    )]
    EdgeIDNotUnique(EdgeID),
}

/// An error occurred while attempting to activate an audio stream in
/// a [`FirewheelCtx`][crate::context::FirewheelCtx].
#[derive(Debug, thiserror::Error)]
pub enum StartStreamError<E: Error> {
    /// An audio stream is already running in this context.
    #[error("Audio stream is already running")]
    AlreadyStarted,
    /// The old audio stream has not finished stopping yet.
    ///
    /// Wait some time and then try starting again.
    ///
    /// Note, in rare cases where the audio thread crashes without cleanly
    /// dropping its contents, this may never succeed. Consider adding a
    /// timeout to avoid deadlocking.
    #[error("Failed to start audio stream: The old audio stream has not finished stopping yet")]
    OldStreamNotFinishedStopping,
    /// The audio graph failed to compile.
    #[error("Failed to start audio stream: Audio graph failed to compile: {0}")]
    GraphCompileError(#[from] CompileGraphError),
    /// A backend-specific error occured.
    #[error("Failed to start audio stream: {0}")]
    BackendError(E),
}

/// An error occured while updating a [`FirewheelCtx`][crate::context::FirewheelCtx].
#[derive(Debug, thiserror::Error)]
pub enum UpdateError<E: Error> {
    /// The context to processor message channel is full.
    #[error("The Firewheel context to processor message channel is full")]
    MsgChannelFull,
    /// The audio graph failed to compile.
    #[error("The audio graph failed to compile: {0}")]
    GraphCompileError(#[from] CompileGraphError),
    /// The audio stream stopped unexpectedly. A new audio stream (even if it's a
    /// dummy audio stream), should be started as soon as possible.
    #[error("The audio stream stopped unexpectedly: {0}")]
    StreamStoppedUnexpectedly(Option<E>),
}
