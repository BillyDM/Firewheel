use firewheel_core::{channel_config::ChannelCount, node::NodeID};

use crate::graph::{Edge, EdgeID, PortIdx};

/// An error occurred while attempting to add an edge to the graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
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

/// An error occurred while attempting to activate a
/// [`FirewheelContext`][crate::context::FirewheelContext].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ActivateError {
    /// The Firewheel context is already active. Either it has never been activated
    /// or the [`FirewheelProcessor`][crate::processor::FirewheelProcessor] counterpart
    /// has not been dropped yet.
    ///
    /// Note, in rare cases where the audio thread crashes without cleanly
    /// dropping its contents, this may never succeed. Consider adding a
    /// timeout to avoid deadlocking.
    #[error("Failed to activate Firewheel context: The Firewheel context is already active")]
    AlreadyActive,
    /// The audio graph failed to compile.
    #[error("Failed to activate Firewheel context: Audio graph failed to compile: {0}")]
    GraphCompileError(#[from] CompileGraphError),
}

/// An error occured while updating a [`FirewheelContext`][crate::context::FirewheelContext].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum UpdateError {
    /// The context to processor message channel is full.
    #[error("The Firewheel context to processor message channel is full")]
    MsgChannelFull,
    /// The audio graph failed to compile.
    #[error("The audio graph failed to compile: {0}")]
    GraphCompileError(#[from] CompileGraphError),
}

/// An error while removing a node in [`FirewheelContext`][crate::context::FirewheelContext].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RemoveNodeError {
    /// Removing the graph in node is not allowed.
    #[error("Removing the graph in node is not allowed")]
    CannotRemoveGraphInNode,
    /// Removing the graph out node is not allowed.
    #[error("Removing the graph out node is not allowed")]
    CannotRemoveGraphOutNode,
}

/// An error occured while deactivate a [`FirewheelContext`][crate::context::FirewheelContext].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum DeactivateError {
    #[error("Timed out waiting for the Firewheel context to deactivate")]
    TimedOut,
}
