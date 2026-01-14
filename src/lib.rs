pub use firewheel_core as core;
pub use firewheel_core::*;
pub use firewheel_graph::*;
pub use firewheel_nodes as nodes;

pub use firewheel_core::dsp::volume::Volume;

#[cfg(feature = "cpal")]
pub use firewheel_cpal as cpal;
#[cfg(feature = "cpal")]
pub type FirewheelContext = FirewheelCtx<self::cpal::CpalBackend>;

#[cfg(feature = "rtaudio")]
pub use firewheel_rtaudio as rtaudio;
#[cfg(all(feature = "rtaudio", not(feature = "cpal")))]
pub type FirewheelContext = FirewheelCtx<self::rtaudio::RtAudioBackend>;

#[cfg(feature = "pool")]
pub use firewheel_pool as pool;

#[cfg(feature = "symphonium")]
pub use firewheel_symphonium::*;
