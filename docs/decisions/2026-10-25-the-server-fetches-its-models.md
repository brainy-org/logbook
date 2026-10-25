# logbook-server fetches libparakeet and its models itself, with no other tool

**Verdict.** On start, logbook-server fetches what it lacks from the
mirror on ghcr.io/brainy-org into `~/.cache/brainy-org`: libparakeet for
this machine and the pinned model files (`crates/engine/src/mirror.rs`).
It speaks the registry's API itself and depends on no program or file of
another tool: no oras, docker or nvidia-smi, no `~/.docker/config.json`.
The only credential is a GitHub token with `read:packages`
(`LOGBOOK_REGISTRY_TOKEN`), needed only while something is fetched.
`scripts/server.sh` now only builds and starts the server.

## Why

The binary alone is what a server, a container or the future desktop app
runs; a shell script with oras beside it is not. The pins (sha256 of each
file, the same as brainy-worker's `models.json`) move from the script into
the code that checks them.

## How

- Every file is a blob fetched by its sha256, hashed as it arrives,
  written beside its place and renamed only once it matches: a file in the
  cache is complete, and is not hashed again at start (1 GB).
- libparakeet goes to `libparakeet-<build>-<sha>/`, so a new release or
  build never mixes with an old one. The build is chosen as the script
  did, except CUDA: NVIDIA's driver is seen through
  `/proc/driver/nvidia/version`, not by running `nvidia-smi`.
- The CUDA build ships cudart and cuBLAS beside libparakeet, which the
  loader finds only through `LD_LIBRARY_PATH`, read when a process starts:
  the server starts itself again with that folder in it (Linux only;
  untested on a CUDA machine).
- `--models` given is used as it is, nothing fetched: a mounted volume may
  be read-only, and a missing file there is an error to see.

## Rejected

- **Models inside the binary**: 1 GB in every build and update, and
  libparakeet loads only from a path, so they would be written out to disk
  at every start anyway.
- **oras, or the Docker config file it writes**: a tool to install, or a
  file another tool owns (and a credential store it may hide behind).
