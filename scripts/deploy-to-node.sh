#!/bin/sh
# Build the current code and install/update it on a hermes-remote Mac. Settings and history are kept.
#   sh scripts/deploy-to-node.sh jpl-graphics-imac
set -eu
node="${1:?usage: deploy-to-node.sh NODE}"
cd "$(dirname "$0")/.."
rm -rf dist && uv build --wheel -q
wheel=$(ls dist/*.whl)
hermes-remote push "$node" "$wheel" "/tmp/$(basename "$wheel")" >/dev/null
hermes-remote push "$node" scripts/install-macos.sh /tmp/install-macos.sh >/dev/null
hermes-remote run "$node" -- /bin/sh -c "sh /tmp/install-macos.sh install /tmp/$(basename "$wheel"); rm -f /tmp/install-macos.sh /tmp/$(basename "$wheel")"
