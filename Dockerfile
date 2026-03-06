# ── Stage 1: Build ────────────────────────────────────────────────────────────
FROM eclipse-temurin:21-jdk-alpine AS builder
WORKDIR /build

# Cache SBT dependencies
COPY project/ project/
COPY build.sbt .
RUN apk add --no-cache bash curl && \
    curl -fL https://github.com/sbt/sbt/releases/download/v1.10.1/sbt-1.10.1.tgz | \
    tar xz -C /usr/local && \
    ln -s /usr/local/sbt/bin/sbt /usr/local/bin/sbt

# Download dependencies first (better Docker layer caching)
RUN sbt update

# Copy source and build
COPY src/ src/
RUN sbt stage

# ── Stage 2: Runtime ──────────────────────────────────────────────────────────
FROM eclipse-temurin:21-jre-alpine
WORKDIR /app

COPY --from=builder /build/target/universal/stage/ .

EXPOSE 8080

ENTRYPOINT ["bin/flashcards"]
