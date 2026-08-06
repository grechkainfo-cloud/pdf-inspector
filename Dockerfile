# syntax=docker/dockerfile:1
#
# pdf-inspector HTTP API — multi-stage build.
#
#   docker build -t pdf-inspector:0.1.7 .
#   docker run -d -p 8080:8080 --name pdf-inspector pdf-inspector:0.1.7
#
# Stage 1 compiles a release binary; stage 2 ships only that binary plus the
# bundled CJK CMaps on a slim Debian base. The result is self-contained: no
# Rust toolchain, no source, no network access needed at runtime.

# ---------------------------------------------------------------------------
# Stage 1 — build
# ---------------------------------------------------------------------------
FROM rust:1-slim-bookworm AS builder

WORKDIR /build

# Strip via rustc rather than binutils — `strip` is not in the slim image.
# Set before the first build so both builds share one dependency cache.
ENV RUSTFLAGS="-C strip=symbols"

# Warm the dependency cache first: with only the manifest present, this layer
# is reused on every rebuild that does not change Cargo.toml, so editing source
# recompiles the crate alone and not the whole dependency tree.
# The bracket glob makes Cargo.lock optional: it pins dependency versions when
# present, and the build still works when it is not checked in.
COPY Cargo.toml Cargo.loc[k] ./
RUN mkdir -p src/bin \
    && : > src/lib.rs \
    && for stub in pdf2md detect_pdf dump_ops pdf_api; do \
    echo 'fn main() {}' > "src/bin/${stub}.rs"; \
    done \
    && cargo build --release --features server --bin pdf-api \
    && rm -rf src

# Real sources. `external/` carries the pdf.js binary CMaps that the extractor
# reads at runtime for CID-keyed CJK fonts.
COPY src ./src
COPY external ./external

# Touch the entry points so cargo notices the swap from stubs to real code.
RUN touch src/lib.rs src/bin/*.rs \
    && cargo build --release --features server \
    --bin pdf-api --bin pdf2md --bin detect-pdf

# ---------------------------------------------------------------------------
# Stage 2 — runtime
# ---------------------------------------------------------------------------
FROM debian:bookworm-slim AS runtime

# curl backs the HEALTHCHECK below; ca-certificates keeps the image usable
# behind TLS-terminating proxies.
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/*

# Unprivileged runtime user. The service writes nothing to disk.
RUN groupadd --system --gid 10001 pdfapi \
    && useradd --system --uid 10001 --gid pdfapi --no-create-home --shell /usr/sbin/nologin pdfapi

COPY --from=builder /build/target/release/pdf-api /usr/local/bin/pdf-api
COPY --from=builder /build/target/release/pdf2md /usr/local/bin/pdf2md
COPY --from=builder /build/target/release/detect-pdf /usr/local/bin/detect-pdf
COPY --from=builder /build/external/bcmaps /opt/pdf-inspector/bcmaps

# CARGO_MANIFEST_DIR does not exist in the runtime image, so point the CMap
# loader at the copied directory explicitly — without it, CID-keyed CJK fonts
# silently lose their Unicode mapping.
ENV PDF_INSPECTOR_BCMAPS_DIR=/opt/pdf-inspector/bcmaps \
    PDF_INSPECTOR_BIND=0.0.0.0:8080 \
    PDF_INSPECTOR_MAX_UPLOAD_MB=64 \
    RUST_LOG=info

EXPOSE 8080
USER 10001:10001

HEALTHCHECK --interval=30s --timeout=5s --start-period=5s --retries=3 \
    CMD curl -fsS "${PDF_INSPECTOR_HEALTH_URL:-http://127.0.0.1:8080/health}" || exit 1

ENTRYPOINT ["/usr/local/bin/pdf-api"]
