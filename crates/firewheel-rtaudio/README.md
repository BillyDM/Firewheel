# firewheel-rtaudio
[RtAudio](https://github.com/thestk/rtaudio) backend for Firewheel (using [RtAudio-rs](https://crates.io/crates/rtaudio))

This backend has better support for "full duplex" audio devices than the default CPAL backend, which allows for less latency between input and output streams. The drawback is that this backend only supports Windows, MacOS, and Linux desktop platforms.

# Prerequisites

`CMake` is required on all platforms.

## Linux

```
apt install cmake pkg-config libasound2-dev libpulse-dev
```

If the `jack_linux` feature is enabled, then also install the jack development headers:
```
apt install libjack-dev
```

## MacOS

### Install CMake: Option 1

Download at https://cmake.org/.

### Install CMake: Option 2

Install with [Homebrew](https://brew.sh/):

```
brew install cmake
```

## Windows

### Install CMake

Download at https://cmake.org/.

# Features

By default, Jack on Linux and ASIO on Windows is disabled. You can enable them with the `jack_linux` and `asio` features.

```
rtaudio = { version = "0.3.2", features = ["jack_linux", "asio"] }
```