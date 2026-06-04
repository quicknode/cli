# syntax=docker/dockerfile:1.7

# We do NOT compile here. The publish-docker workflow pulls the
# cross-compiled musl binary from the GitHub release artifacts and
# stages it at ./qn before invoking `docker buildx build`. That keeps
# the multi-arch image build a no-op COPY rather than a cargo rebuild
# (the SLSA-attested binary in the release IS what ships in the image).
ARG TARGETARCH

FROM gcr.io/distroless/static-debian12:nonroot
LABEL org.opencontainers.image.source="https://github.com/quicknode/cli"
LABEL org.opencontainers.image.description="qn — Quicknode CLI"
LABEL org.opencontainers.image.licenses="MIT"

COPY --chown=nonroot:nonroot qn /qn
USER nonroot
ENTRYPOINT ["/qn"]
