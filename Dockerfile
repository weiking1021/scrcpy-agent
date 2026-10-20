# Builds and checks scrcpy-agent on Linux (x86_64), without a Linux machine of your own.
#
#   Checks only (clippy + unit tests):   docker build --target check .
#   The program (a Linux executable):    docker build --target export --output type=local,dest=out .
#                                        -> out/scrcpy-agent
#   A .deb package:                      docker build --target export-deb --output type=local,dest=out .
#                                        -> out/scrcpy-agent_<version>_amd64.deb
#   Both (one build):                    docker build --target export-linux --output type=local,dest=out .
#   Everything, with the Windows packages too, in one command: `npm run package` (scripts/package.mjs,
#   which asks for `export-linux` and `export-windows`: keep its list in step with the targets here).
#
# The executable needs the system libraries of Tauri on the machine that runs it
# (WebKitGTK 4.1 and the Ayatana app indicator; see "runtime" below).

FROM rust:1-bookworm AS base

# The system libraries Tauri 2 needs to build on Debian/Ubuntu.
RUN apt-get update && apt-get install -y --no-install-recommends \
      build-essential curl wget file pkg-config libssl-dev libxdo-dev \
      libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev \
 && rm -rf /var/lib/apt/lists/*

# Node.js (LTS) for the web part.
RUN curl -fsSL https://deb.nodesource.com/setup_22.x | bash - \
 && apt-get install -y --no-install-recommends nodejs \
 && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# The packages first, so a change of source does not fetch them again.
COPY package.json package-lock.json ./
RUN npm ci
COPY . .
RUN npm run build

# ---------------------------------------------------------------- checks
FROM base AS check
RUN rustup component add clippy
WORKDIR /app/src-tauri
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/cache/target \
    CARGO_TARGET_DIR=/cache/target cargo clippy --all-targets \
 && CARGO_TARGET_DIR=/cache/target cargo test

# ---------------------------------------------------------------- the program and the .deb package
# One build gives both: `tauri build` is the build that has the web part inside the
# executable (a plain `cargo build --release` does not), and the release build with
# its link-time optimization is the slow part, so it is made once.
FROM base AS deb
# (The folder of the packages lives in the build cache and keeps what earlier versions
# made; it is emptied first so only this version's package is exported.)
WORKDIR /app
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/cache/target \
    rm -rf /cache/target/release/bundle \
 && CARGO_TARGET_DIR=/cache/target npx tauri build --bundles deb \
 && mkdir /packages /program \
 && cp /cache/target/release/bundle/deb/*.deb /packages/ \
 && cp /cache/target/release/scrcpy-agent /program/

FROM scratch AS export
COPY --from=deb /program /

FROM scratch AS export-deb
COPY --from=deb /packages /

# Both, in one build (what `npm run package` asks for).
FROM scratch AS export-linux
COPY --from=deb /program /
COPY --from=deb /packages /

# ---------------------------------------------------------------- Windows (cross-compiled)
# An NSIS installer and a portable zip for Windows x86_64, built here on Linux with cargo-xwin
# (which fetches the Windows SDK). There is no .msi: its tool (WiX) only runs on Windows.
# The result is only known to compile; it cannot be run here, so it has to be tried on Windows.
# No signing (a decision of the project).
#
#   docker build --target export-windows --output type=local,dest=out .
#     -> out/scrcpy-agent_<version>_x64-setup.exe, out/scrcpy-agent_<version>_x64_portable.zip
# Its own base (Debian trixie): the NSIS of bookworm (3.08) lacks an include file that the
# installer script of Tauri needs (Win\RestartManager.nsh); trixie has 3.11.
FROM rust:1-trixie AS windows
RUN apt-get update && apt-get install -y --no-install-recommends \
      build-essential curl ca-certificates pkg-config nsis lld llvm clang zip \
      libayatana-appindicator3-dev \
 && rm -rf /var/lib/apt/lists/*
# (The Tauri command line checks for an app indicator library on the machine it runs on,
# whatever it builds for.)
RUN curl -fsSL https://deb.nodesource.com/setup_22.x | bash - \
 && apt-get install -y --no-install-recommends nodejs \
 && rm -rf /var/lib/apt/lists/*
RUN rustup target add x86_64-pc-windows-msvc \
 && cargo install --locked cargo-xwin
WORKDIR /app
COPY package.json package-lock.json ./
RUN npm ci
COPY . .
ENV XWIN_ACCEPT_LICENSE=1
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/cache/target-windows \
    --mount=type=cache,target=/root/.cache/cargo-xwin \
    rm -rf /cache/target-windows/x86_64-pc-windows-msvc/release/bundle \
 && CARGO_TARGET_DIR=/cache/target-windows npx tauri build --runner cargo-xwin --target x86_64-pc-windows-msvc --bundles nsis \
 && out=/cache/target-windows/x86_64-pc-windows-msvc/release \
 && version=$(cargo metadata --no-deps --format-version 1 --manifest-path src-tauri/Cargo.toml | node -p "JSON.parse(require('fs').readFileSync(0, 'utf8')).packages[0].version") \
 && mkdir -p /packages /portable \
 && cp "$out"/bundle/nsis/*-setup.exe /packages/ \
 && cp "$out/scrcpy-agent.exe" /portable/ \
 && touch /portable/portable.txt \
 && (cd /portable && zip -q "/packages/scrcpy-agent_${version}_x64_portable.zip" scrcpy-agent.exe portable.txt)

FROM scratch AS export-windows
COPY --from=windows /packages /

# What a machine needs to run it (Debian/Ubuntu names):
#   libwebkit2gtk-4.1-0 libayatana-appindicator3-1 libgtk-3-0
