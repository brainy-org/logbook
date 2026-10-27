# logbook-server: live transcription over WebSocket, final passes, reports
# (docs/protocol.md), in one image for the CPU and for an NVIDIA GPU.
#
#   docker build -t logbook-server .
#   docker run --rm -p 3300:3300 -v logbook-cache:/home/logbook/.cache/brainy-org \
#       -v logbook-data:/data logbook-server                       # CPU
#   docker run ... --gpus all logbook-server                         # NVIDIA GPU
#
# Or compose.yaml (and compose.gpu.yaml). One image for both: libparakeet
# and the models are not in it. The server fetches them at its first start
# from the mirror on ghcr.io/brainy-org, by sha256, into the cache volume
# (crates/engine/src/mirror.rs), and picks the CUDA build when it is given
# the GPU (/dev/nvidiactl), the CPU build otherwise. The CUDA build carries
# its own cudart and cuBLAS: the host gives only the driver, through the
# NVIDIA container toolkit. README.md, "Docker".

ARG RUST_IMAGE=rust:1.92.0-slim-bookworm
ARG DEBIAN_IMAGE=debian:bookworm-slim

# ---------------------------------------------------------------- build
FROM ${RUST_IMAGE} AS build
# cmake and a C compiler: the opus crate builds libopus, ring its own C.
RUN apt-get update \
    && apt-get install -y --no-install-recommends build-essential cmake pkg-config \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /src
# The whole workspace: cargo reads every member's manifest, the desktop
# app's included, even to build one package.
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p logbook-server \
    && cp target/release/logbook-server /usr/local/bin/logbook-server

# ---------------------------------------------------------------- runtime
FROM ${DEBIAN_IMAGE} AS runtime
# ca-certificates: the mirror and the language model's API over HTTPS.
# libgomp1: libparakeet's CPU builds use OpenMP.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates libgomp1 \
    && rm -rf /var/lib/apt/lists/*
RUN useradd --system --uid 10001 --create-home --home-dir /home/logbook logbook \
    && install -d -o logbook -g logbook /data /home/logbook/.cache/brainy-org
COPY --from=build /usr/local/bin/logbook-server /usr/local/bin/logbook-server
ENV LOGBOOK_LISTEN=0.0.0.0:3300 \
    # The people's tokens (their hashes) and admin.token.
    LOGBOOK_DATA=/data \
    # Asked for only when the container is given a GPU (--gpus): the
    # driver's compute libraries, nothing of the display.
    NVIDIA_DRIVER_CAPABILITIES=compute,utility \
    # glibc gives every thread an arena that keeps what it frees; the
    # server has a thread per session.
    MALLOC_ARENA_MAX=2
USER logbook
WORKDIR /home/logbook
# libparakeet and the models (about 1.1 GB, 1.6 GB with the CUDA build),
# fetched once; the tokens.
VOLUME ["/home/logbook/.cache/brainy-org", "/data"]
EXPOSE 3300
ENTRYPOINT ["logbook-server"]
