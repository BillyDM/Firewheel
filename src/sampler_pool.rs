use core::num::NonZeroU32;

use firewheel_core::{
    channel_config::NonZeroChannelCount,
    clock::EventDelay,
    diff::{Diff, PathBuilder},
    node::NodeID,
};
use firewheel_cpal::FirewheelContext;
use firewheel_nodes::sampler::{PlaybackState, SamplerConfig, SamplerNode, SamplerState};
use smallvec::SmallVec;
use thunderdome::Arena;

pub trait FxChain: Default {
    /// Construct the nodes in the FX chain and connect them, returning a list of the
    /// new node ids.
    ///
    /// * `sampler_node_id` - The ID of the sampler node in this worker instance.
    /// * `sampler_num_channels` - The number of channels in the sampler node.
    /// * `dst_node_id` - The ID of the node that the last node in this FX chain should
    /// connect to.
    /// * `dst_num_channels` - The number of input channels on `dst_node_id`.
    /// * `cx` - The firewheel context.
    fn construct_and_connect(
        &mut self,
        sampler_node_id: NodeID,
        sampler_num_channels: NonZeroChannelCount,
        dst_node_id: NodeID,
        dst_num_channels: NonZeroChannelCount,
        cx: &mut FirewheelContext,
    ) -> Vec<NodeID>;
}

struct Worker<FX: FxChain> {
    params: SamplerNode,
    sampler_id: NodeID,

    fx_state: FxChainState<FX>,

    assigned_worker_id: Option<WorkerID>,
}

pub struct FxChainState<FX: FxChain> {
    pub fx_chain: FX,
    pub node_ids: Vec<NodeID>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WorkerID(thunderdome::Index);

impl WorkerID {
    pub const DANGLING: Self = Self(thunderdome::Index::DANGLING);
}

impl Default for WorkerID {
    fn default() -> Self {
        Self::DANGLING
    }
}

/// A pool of sampler nodes that can dynamically be assigned work.
///
/// Each worker also contains its own chain of effect nodes.
pub struct SamplerPool<FX: FxChain> {
    workers: Vec<Worker<FX>>,
    worker_ids: Arena<usize>,
    num_active_workers: usize,
}

impl<FX: FxChain> SamplerPool<FX> {
    /// Construct a new sampler pool.
    ///
    /// * `num_workers` - The total number of workers that can work in parallel. More workers
    /// will allow more samples to be played concurrently, but will also increase processing
    /// overhead. A value of `16` is a good place to start.
    /// * `config` - The configuration of the sampler nodes.
    /// * `dst_node_id` - The ID of the node that the last effect in each fx chain instance
    /// will connect to.
    /// * `dst_num_channels` - The number of input channels in `dst_node_id`.
    /// * `cx` - The firewheel context.
    pub fn new(
        num_workers: usize,
        config: SamplerConfig,
        dst_node_id: NodeID,
        dst_num_channels: NonZeroChannelCount,
        cx: &mut FirewheelContext,
    ) -> Self {
        assert_ne!(num_workers, 0);

        Self {
            workers: (0..num_workers)
                .map(|_| {
                    let sampler_node = SamplerNode::default();

                    let sampler_id = cx.add_node(sampler_node.clone(), Some(config.clone()));

                    let mut fx_chain = FX::default();

                    let fx_ids = fx_chain.construct_and_connect(
                        sampler_id,
                        config.channels,
                        dst_node_id,
                        dst_num_channels,
                        cx,
                    );

                    Worker {
                        params: sampler_node,
                        sampler_id,

                        fx_state: FxChainState {
                            fx_chain,
                            node_ids: fx_ids,
                        },

                        assigned_worker_id: None,
                    }
                })
                .collect(),
            worker_ids: Arena::with_capacity(num_workers),
            num_active_workers: 0,
        }
    }

    pub fn num_workers(&self) -> usize {
        self.workers.len()
    }

    /// Queue a new work to play a sequence.
    ///
    /// * `params` - The parameters of the sequence to play.
    /// * `steal` - If this is `true`, then if there are no more workers left in
    /// in the pool, the oldest one will be stopped and replaced with this new
    /// one. If this is `false`, then an error will be returned if no more workers
    /// are left.
    /// * `cx` - The Firewheel context.
    /// * `fx_chain` - A closure to add additional nodes to this worker instance.
    ///
    /// This will return an error if `params.playback == PlaybackState::Stop`.
    pub fn new_worker(
        &mut self,
        params: &SamplerNode,
        steal: bool,
        cx: &mut FirewheelContext,
        fx_chain: impl FnOnce(&mut FxChainState<FX>, &mut FirewheelContext),
    ) -> Result<NewWorkerResult, NewWorkerError> {
        if *params.playback == PlaybackState::Stop {
            return Err(NewWorkerError::PlaybackStateIsStop);
        }

        if !steal && self.num_active_workers == self.workers.len() {
            return Err(NewWorkerError::NoMoreWorkers);
        }

        let mut idx = 0;
        let mut max_score = 0;
        for (i, worker) in self.workers.iter().enumerate() {
            if worker.assigned_worker_id.is_none() {
                idx = i;
                break;
            }

            let score = cx
                .node_state::<SamplerState>(worker.sampler_id)
                .unwrap()
                .worker_score(&worker.params);

            if score == u64::MAX {
                idx = i;
                break;
            }

            if score > max_score {
                max_score = score;
                idx = i;
            }
        }

        let worker_id = WorkerID(self.worker_ids.insert(idx));

        let worker = &mut self.workers[idx];

        let old_worker_id = worker.assigned_worker_id.take();
        let was_playing_sequence = if let Some(old_worker_id) = old_worker_id {
            self.worker_ids.remove(old_worker_id.0);

            !(*worker.params.playback == PlaybackState::Stop
                || cx
                    .node_state::<SamplerState>(worker.sampler_id)
                    .unwrap()
                    .stopped())
        } else {
            false
        };

        worker.assigned_worker_id = Some(worker_id);
        self.num_active_workers += 1;

        let mut event_queue = cx.event_queue(worker.sampler_id);
        params.diff(&worker.params, PathBuilder::default(), &mut event_queue);

        worker.params = params.clone();

        cx.node_state::<SamplerState>(worker.sampler_id)
            .unwrap()
            .mark_stopped(false);

        (fx_chain)(&mut worker.fx_state, cx);

        Ok(NewWorkerResult {
            worker_id,
            old_worker_id,
            was_playing_sequence,
        })
    }

    /// Sync the parameters for the given worker.
    ///
    /// If `params.playback == PlaybackState::Stop`, then this worker will be removed
    /// and the `worker_id` will be invalidated.
    ///
    /// Returns `true` if a worker with the given ID exists, `false` otherwise.
    pub fn sync_worker_params(
        &mut self,
        worker_id: WorkerID,
        params: &SamplerNode,
        cx: &mut FirewheelContext,
    ) -> bool {
        let Some(idx) = self.worker_ids.get(worker_id.0).copied() else {
            return false;
        };

        let worker = &mut self.workers[idx];

        let mut event_queue = cx.event_queue(worker.sampler_id);
        params.diff(&worker.params, PathBuilder::default(), &mut event_queue);

        worker.params = params.clone();

        if *worker.params.playback == PlaybackState::Stop {
            self.worker_ids.remove(worker_id.0);
            worker.assigned_worker_id = None;
            self.num_active_workers -= 1;
        }

        true
    }

    /// Pause the given worker.
    ///
    /// Returns `true` if a worker with the given ID exists, `false` otherwise.
    pub fn pause(&mut self, worker_id: WorkerID, cx: &mut FirewheelContext) -> bool {
        let Some(idx) = self.worker_ids.get(worker_id.0).copied() else {
            return false;
        };

        let worker = &mut self.workers[idx];

        worker.params.pause();
        cx.queue_event_for(worker.sampler_id, worker.params.sync_playback_event());

        true
    }

    /// Resume the given worker.
    ///
    /// Returns `true` if a worker with the given ID exists, `false` otherwise.
    pub fn resume(
        &mut self,
        worker_id: WorkerID,
        delay: Option<EventDelay>,
        cx: &mut FirewheelContext,
    ) -> bool {
        let Some(idx) = self.worker_ids.get(worker_id.0).copied() else {
            return false;
        };

        let worker = &mut self.workers[idx];

        worker.params.resume(delay);
        cx.queue_event_for(worker.sampler_id, worker.params.sync_playback_event());

        true
    }

    /// Stop the given worker.
    ///
    /// This will remove the worker and invalidate the given `worker_id`.
    ///
    /// Returns `true` if a worker with the given ID exists and was stopped.
    pub fn stop(&mut self, worker_id: WorkerID, cx: &mut FirewheelContext) -> bool {
        let Some(idx) = self.worker_ids.get(worker_id.0).copied() else {
            return false;
        };

        let worker = &mut self.workers[idx];

        worker.params.stop();
        cx.queue_event_for(worker.sampler_id, worker.params.sync_playback_event());

        self.worker_ids.remove(worker_id.0);
        worker.assigned_worker_id = None;
        self.num_active_workers -= 1;

        true
    }

    /// Pause all workers.
    pub fn pause_all(&mut self, cx: &mut FirewheelContext) {
        for worker in self.workers.iter_mut() {
            worker.params.pause();
            if worker.assigned_worker_id.is_some() {
                *worker.params.playback = PlaybackState::Pause;
                cx.queue_event_for(worker.sampler_id, worker.params.sync_playback_event());
            }
        }
    }

    /// Resume all workers.
    pub fn resume_all(&mut self, delay: Option<EventDelay>, cx: &mut FirewheelContext) {
        for worker in self.workers.iter_mut() {
            if worker.assigned_worker_id.is_some() {
                worker.params.resume(delay);
                cx.queue_event_for(worker.sampler_id, worker.params.sync_playback_event());
            }
        }
    }

    /// Stop all workers.
    pub fn stop_all(&mut self, cx: &mut FirewheelContext) {
        for worker in self.workers.iter_mut() {
            if let Some(_) = worker.assigned_worker_id.take() {
                worker.params.stop();
                cx.queue_event_for(worker.sampler_id, worker.params.sync_playback_event());
            }
        }

        self.worker_ids.clear();
        self.num_active_workers = 0;
    }

    pub fn sampler_node(&self, worker_id: WorkerID) -> Option<&SamplerNode> {
        if let Some(idx) = self.worker_ids.get(worker_id.0).copied() {
            Some(&self.workers[idx].params)
        } else {
            None
        }
    }

    pub fn fx_chain(&self, worker_id: WorkerID) -> Option<&FxChainState<FX>> {
        if let Some(idx) = self.worker_ids.get(worker_id.0).copied() {
            Some(&self.workers[idx].fx_state)
        } else {
            None
        }
    }

    pub fn fx_chain_mut(&mut self, worker_id: WorkerID) -> Option<&mut FxChainState<FX>> {
        if let Some(idx) = self.worker_ids.get(worker_id.0).copied() {
            Some(&mut self.workers[idx].fx_state)
        } else {
            None
        }
    }

    /// Returns `true` if the sequence has either not started playing yet or has finished
    /// playing.
    pub fn stopped(&self, worker_id: WorkerID, cx: &FirewheelContext) -> bool {
        if let Some(idx) = self.worker_ids.get(worker_id.0).copied() {
            cx.node_state::<SamplerState>(self.workers[idx].sampler_id)
                .unwrap()
                .stopped()
        } else {
            true
        }
    }

    /// Get the current playhead for the given worker in units of seconds.
    ///
    /// Returns `None` if a worker with the given ID does not exist.
    pub fn playhead_seconds(
        &mut self,
        worker_id: WorkerID,
        sample_rate: NonZeroU32,
        cx: &FirewheelContext,
    ) -> Option<f64> {
        self.worker_ids.get(worker_id.0).copied().map(|idx| {
            cx.node_state::<SamplerState>(self.workers[idx].sampler_id)
                .unwrap()
                .playhead_seconds(sample_rate)
        })
    }

    /// Get the current playhead for the given worker in units of frames (samples of a
    /// single channel of audio).
    ///
    /// Returns `None` if a worker with the given ID does not exist.
    pub fn playhead_frames(&mut self, worker_id: WorkerID, cx: &FirewheelContext) -> Option<u64> {
        self.worker_ids.get(worker_id.0).copied().map(|idx| {
            cx.node_state::<SamplerState>(self.workers[idx].sampler_id)
                .unwrap()
                .playhead_frames()
        })
    }

    /// Poll for the current number of active workers, and return a list of
    /// workers which have finished playing.
    ///
    /// Calling this method is optional.
    pub fn poll(&mut self, cx: &FirewheelContext) -> PollResult {
        self.num_active_workers = 0;
        let mut finished_workers = SmallVec::new();

        for worker in self.workers.iter_mut() {
            if worker.assigned_worker_id.is_some() {
                if cx
                    .node_state::<SamplerState>(worker.sampler_id)
                    .unwrap()
                    .stopped()
                {
                    let id = worker.assigned_worker_id.take().unwrap();
                    self.worker_ids.remove(id.0);
                    finished_workers.push(id);
                } else {
                    self.num_active_workers += 1;
                }
            }
        }

        PollResult { finished_workers }
    }

    /// The total number of active workers.
    pub fn num_active_workers(&self) -> usize {
        self.num_active_workers
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PollResult {
    /// The worker IDs which have finished playing. These IDs are now
    /// invalidated.
    pub finished_workers: SmallVec<[WorkerID; 4]>,
}

/// The result of calling [`SamplerPool::new_worker`].
pub struct NewWorkerResult {
    /// The new ID of the worker assigned to play this sequence.
    pub worker_id: WorkerID,

    /// The ID that was previously assigned to this worker.
    pub old_worker_id: Option<WorkerID>,

    /// If this is `true`, then this worker was already playing a sequence, and that
    /// sequence has been stopped.
    pub was_playing_sequence: bool,
}

/// A default [`SamplerPool`] [`FxChain`] for 2D game audio.
///
/// This chain contains a single `VolumePan` node.
#[derive(Default, Debug, Clone, Copy, PartialEq)]
pub struct VolumePanChain {
    pub volume_pan: firewheel_nodes::volume_pan::VolumePanNode,
    pub config: firewheel_nodes::volume_pan::VolumeNodeConfig,
}

impl VolumePanChain {
    pub fn set_params(
        &mut self,
        params: firewheel_nodes::volume_pan::VolumePanNode,
        node_ids: &[NodeID],
        cx: &mut FirewheelContext,
    ) {
        let node_id = node_ids[0];

        self.volume_pan.diff(
            &params,
            PathBuilder::default(),
            &mut cx.event_queue(node_id),
        );
    }
}

impl FxChain for VolumePanChain {
    fn construct_and_connect(
        &mut self,
        sampler_node_id: NodeID,
        sampler_num_channels: NonZeroChannelCount,
        dst_node_id: NodeID,
        dst_num_channels: NonZeroChannelCount,
        cx: &mut FirewheelContext,
    ) -> Vec<NodeID> {
        let volume_pan_params = firewheel_nodes::volume_pan::VolumePanNode::default();

        let volume_pan_node_id = cx.add_node(volume_pan_params, Some(self.config));

        cx.connect(
            sampler_node_id,
            volume_pan_node_id,
            if sampler_num_channels.get().get() == 1 {
                &[(0, 0), (0, 1)]
            } else {
                &[(0, 0), (1, 1)]
            },
            false,
        )
        .unwrap();

        cx.connect(
            volume_pan_node_id,
            dst_node_id,
            if dst_num_channels.get().get() == 1 {
                &[(0, 0), (1, 0)]
            } else {
                &[(0, 0), (1, 1)]
            },
            false,
        )
        .unwrap();

        vec![volume_pan_node_id]
    }
}

/// A default [`SamplerPool`] [`FxChain`] for 3D game audio.
///
/// This chain contains a single `SpatialBasic` node.
#[cfg(feature = "spatial_basic_node")]
#[derive(Default, Debug, Clone, Copy, PartialEq)]
pub struct SpatialBasicChain {
    pub spatial_basic: firewheel_nodes::spatial_basic::SpatialBasicNode,
    pub config: firewheel_nodes::spatial_basic::SpatialBasicConfig,
}

#[cfg(feature = "spatial_basic_node")]
impl SpatialBasicChain {
    pub fn set_params(
        &mut self,
        params: firewheel_nodes::spatial_basic::SpatialBasicNode,
        node_ids: &[NodeID],
        cx: &mut FirewheelContext,
    ) {
        let node_id = node_ids[0];

        self.spatial_basic.diff(
            &params,
            PathBuilder::default(),
            &mut cx.event_queue(node_id),
        );
    }
}

#[cfg(feature = "spatial_basic_node")]
impl FxChain for SpatialBasicChain {
    fn construct_and_connect(
        &mut self,
        sampler_node_id: NodeID,
        sampler_num_channels: NonZeroChannelCount,
        dst_node_id: NodeID,
        dst_num_channels: NonZeroChannelCount,
        cx: &mut FirewheelContext,
    ) -> Vec<NodeID> {
        let spatial_basic_params = firewheel_nodes::spatial_basic::SpatialBasicNode::default();

        let spatial_basic_node_id = cx.add_node(spatial_basic_params, Some(self.config));

        cx.connect(
            sampler_node_id,
            spatial_basic_node_id,
            if sampler_num_channels.get().get() == 1 {
                &[(0, 0), (0, 1)]
            } else {
                &[(0, 0), (1, 1)]
            },
            false,
        )
        .unwrap();

        cx.connect(
            spatial_basic_node_id,
            dst_node_id,
            if dst_num_channels.get().get() == 1 {
                &[(0, 0), (1, 0)]
            } else {
                &[(0, 0), (1, 1)]
            },
            false,
        )
        .unwrap();

        vec![spatial_basic_node_id]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum NewWorkerError {
    #[error("Could not create new sampler pool worker: the given playback state was PlaybackState::Stop")]
    PlaybackStateIsStop,
    #[error("Could not create new sampler pool worker: the worker pool is full")]
    NoMoreWorkers,
}
