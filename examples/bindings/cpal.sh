#!/bin/bash
set -e

# Change to the project root directory
cd "$(dirname "$0")/../.."

# Build the Rust library
cargo build --package firewheel-c --features cpal

# Compile the C example
gcc examples/bindings/main.c -Icrates/firewheel-c/include -Ltarget/debug -lfirewheel_c -lasound -ludev -lpthread -lm -o examples/bindings/c_cpal_example

# Run the example
./examples/bindings/c_cpal_example
