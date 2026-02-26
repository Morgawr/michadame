#!/bin/bash

# Ensure cargo-flamegraph is installed
if ! command -v cargo-flamegraph &> /dev/null
then
    echo "cargo-flamegraph could not be found, installing..."
    cargo install flamegraph
fi

echo "Starting profiling. Close the application gracefully to generate the flamegraph."
cargo flamegraph --profile profiling
echo "Profiling complete. Output saved to flamegraph.svg"
