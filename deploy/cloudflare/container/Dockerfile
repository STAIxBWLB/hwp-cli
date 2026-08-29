# hwp MCP session container.
#
# Runs `hwp serve` — the HTTP adapter over the same protocol core the stdio
# server uses (docs/design/22-remote-mcp-deployment.md §3.2). There is no shell,
# no bridge and no second process: the Worker is the only thing that can reach
# this port, and `--root` confines every file the tools touch.
#
# Built from repository source because `hwp serve` is newer than the latest
# release. Once a release carries it, replace the build stage with a
# checksum-verified release tarball on the slim base — that cuts the image and
# the cold start considerably. See ../README.md.

FROM rust:1.93-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release -p hwp-cli --bin hwp

FROM debian:bookworm-slim
# Rendering and PDF output need real font files; fonts-nanum matches the font
# baseline CI already uses (glyf outlines, fast in debug builds).
RUN apt-get update \
 && apt-get install -y --no-install-recommends fonts-nanum \
 && rm -rf /var/lib/apt/lists/*

COPY --from=build /src/target/release/hwp /usr/local/bin/hwp

RUN useradd --create-home --uid 10001 hwp \
 && mkdir -p /work \
 && chown hwp:hwp /work
USER hwp
WORKDIR /work

# No init: `hwp serve` handles SIGTERM itself, so it stops correctly even as PID 1,
# where the kernel delivers no signal that still has its default disposition.
EXPOSE 8080
CMD ["hwp", "serve", \
     "--addr", "0.0.0.0:8080", \
     "--root", "/work", \
     "--files", \
     "--font-dir", "/usr/share/fonts/truetype/nanum"]
