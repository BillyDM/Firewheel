<div align="center"><img src="./assets/logo-512.png" width="64px" height="64px"/><h1>Firewheel</h1></div>

[![Documentation](https://docs.rs/firewheel/badge.svg)](https://docs.rs/firewheel)
[![Crates.io](https://img.shields.io/crates/v/firewheel.svg)](https://crates.io/crates/firewheel)
[![License](https://img.shields.io/crates/l/firewheel.svg)](https://github.com/BillyDM/firewheel/blob/main/LICENSE-APACHE)

Firewheel is a fully-featured libre open source audio graph engine for games and other applications!

## Key Features

* Modular design that can be run on any backend that provides an audio stream
    * Included backends supporting Windows, Mac, Linux, Android, iOS, and WebAssembly
* Flexible audio graph engine (supports any directed, acyclic graph with support for both one-to-many and many-to-one connections)
* A suite of essential built-in audio nodes
* Custom audio node API allowing for a plethora of 3rd party generators and effects
* Basic [CLAP] plugin hosting (non-WASM only), allowing for more open source and proprietary 3rd party effects and synths
* Silence optimizations (avoid processing if the audio buffer contains all zeros, useful when using "pools" of nodes where the majority of the time nodes are unused)
* Ability to add "sequences" to certain nodes (i.e. automation and sequences of events)
* Support for loading a wide variety of audio files using [Symphonium]
* Fault tolerance for audio streams (The game shouldn't stop or crash just because the player accidentally unplugged their headphones.)
* Properly respect realtime constraints (no mutexes!)

## Roadmap

✅ = complete, 🚧 = partially complete, ⬛ = Not implemented yet, ❔= might implement

| Feature                                               | Status                                               |
| ----------------------------------------------------- | ---------------------------------------------------- |
| Core audio graph engine                               | ✅                                                   |
| 3rd party plugin API                                  | ✅                                                   |
| [CPAL] audio backend                                  | 🚧 (audio output works, audio input WIP)             |
| Loading audio files with [Symphonium]                 | ✅                                                   |
| Volume node                                           | ✅                                                   |
| VolumePan node                                        | ✅                                                   |
| Stereo to mono node                                   | ✅                                                   |
| Peak meter node                                       | ✅                                                   |
| Beep test node                                        | ✅                                                   |
| Sampler node                                          | 🚧 (one-shot works, pitch shift WIP, sequencer WIP)  |
| Basic spatial positioning node                        | ✅                                                   |
| Input stream node (stream audio into the graph)       | ⬛                                                   |
| Output stream node (stream audio out of the graph)    | ⬛                                                   |
| Blending sampler node (blend between music tracks)    | ⬛                                                   |
| Disk streaming SampleResource (using [creek])         | ⬛                                                   |
| Network streaming SampleResource                      | ❔ (only if demand is there)                         |
| Filter effect node (LP, HP, BP)                       | ⬛                                                   |
| Convolution node (apply IR effects like reverb)       | ⬛                                                   |
| Echo effect node                                      | ⬛                                                   |
| [CLAP] plugin node                                    | ⬛                                                   |
| Delay compensation node                               | ⬛                                                   |
| Advanced spatial positioning node                     | ❔ (help from DSP expert needed)                     |
| [RtAudio] backend                                     | ⬛                                                   |
| [Interflow] backend                                   | ⬛                                                   |
| C bindings                                            | ❔ (only if demand is there)                         |

## Motivation

While Firewheel is its own standalone project, we are also working closely with the [Bevy](https://bevyengine.org/) game engine to make it Bevy's default audio engine.

## Get Involved

Join the discussion in the [Firewheel Discord Server](https://discord.gg/rKzZpjGCGs) or in the [Bevy Discord Server](https://discord.gg/bevy) under the `working-groups -> Better Audio` channel!

If you are interested in contributing code, first read the [Design Document] and then visit the [Project Board](https://github.com/users/BillyDM/projects/1).

If you are a game or other app developer that wishes to see this project flourish, please consider donating or sponsoring! Links are on the right side of the GitHub page. 🌼

## License

Licensed under either of

* Apache License, Version 2.0, (LICENSE-APACHE or http://www.apache.org/licenses/LICENSE-2.0), or
* MIT license (LICENSE-MIT or http://opensource.org/licenses/MIT)

at your option.

[Design Document]: DESIGN_DOC.md
[CPAL]: https://github.com/RustAudio/cpal
[Symphonium]: https://github.com/MeadowlarkDAW/symphonium
[creek]: https://github.com/MeadowlarkDAW/creek
[CLAP]: https://cleveraudio.org/
[RtAudio]: https://github.com/BillyDM/rtaudio-rs
[Interflow]: https://github.com/SolarLiner/interflow
