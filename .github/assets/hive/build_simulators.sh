#!/usr/bin/env bash
set -euo pipefail

# Resolve repo root no matter where the script is called from
ROOT_DIR="$(git rev-parse --show-toplevel)"
ASSETS_DIR="$ROOT_DIR/hive_assets"
HIVETESTS_DIR="$ROOT_DIR/hivetests"

# Ensure hive_assets exists
mkdir -p "$ASSETS_DIR"

cd "$HIVETESTS_DIR"
go build .

# Build and cache reth client with one sim
./hive -client reth --sim devp2p --sim.timelimit 1s || true

# Run each hive command in the background for each simulator and wait
echo "Building images"
./hive -client reth --sim "ethereum/eest" \
  --sim.buildarg GIT_URL=https://github.com/fselmo/execution-spec-tests.git \
  --sim.buildarg GIT_REF=feat/amsterdam-and-block-access-lists \
  --sim.timelimit 1s || true &

./hive -client reth --sim "devp2p" --sim.timelimit 1s || true &
./hive -client reth --sim "ethereum/rpc-compat" --sim.timelimit 1s || true &
./hive -client reth --sim "smoke/genesis" --sim.timelimit 1s || true &
./hive -client reth --sim "smoke/network" --sim.timelimit 1s || true &
./hive -client reth --sim "ethereum/sync" --sim.timelimit 1s || true &
wait

# Run docker save in parallel, wait and exit on error
echo "Saving images"
saving_pids=( )
docker save hive/hiveproxy:latest -o "$ASSETS_DIR/hiveproxy.tar" & saving_pids+=( $! )
docker save hive/simulators/devp2p:latest -o "$ASSETS_DIR/devp2p.tar" & saving_pids+=( $! )
docker save hive/simulators/ethereum/engine:latest -o "$ASSETS_DIR/engine.tar" & saving_pids+=( $! )
docker save hive/simulators/ethereum/rpc-compat:latest -o "$ASSETS_DIR/rpc_compat.tar" & saving_pids+=( $! )
docker save hive/simulators/ethereum/eest/consume-engine:latest -o "$ASSETS_DIR/eest_engine.tar" & saving_pids+=( $! )
docker save hive/simulators/ethereum/eest/consume-rlp:latest -o "$ASSETS_DIR/eest_rlp.tar" & saving_pids+=( $! )
docker save hive/simulators/smoke/genesis:latest -o "$ASSETS_DIR/smoke_genesis.tar" & saving_pids+=( $! )
docker save hive/simulators/smoke/network:latest -o "$ASSETS_DIR/smoke_network.tar" & saving_pids+=( $! )
docker save hive/simulators/ethereum/sync:latest -o "$ASSETS_DIR/ethereum_sync.tar" & saving_pids+=( $! )
for pid in "${saving_pids[@]}"; do
    wait "$pid" || exit
done

# Prevent CI jobs from rebuilding images
git apply "$ROOT_DIR/.github/assets/hive/no_sim_build.diff"
go build .

# Move final hive binary to assets
mv ./hive "$ASSETS_DIR/"